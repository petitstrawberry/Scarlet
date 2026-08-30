//! Pure-Rust SSH server for Scarlet.
//!
//! The transport and cryptography are provided by Sunset. Scarlet's native
//! `GetRandom` syscall backs the Rust `getrandom` ecosystem, and authenticated
//! SSH sessions are bridged to a Scarlet PTY running the system shell.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::{num::NonZeroU32, time::Duration};

use hmac::{Hmac, Mac};
use rand_core::OsRng;
use sha2::Sha256;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey};
use std::{
    env, format,
    fs::{File, create_directory},
    handle::Handle,
    io::{ErrorKind, Read, Write},
    println,
    pty::{PtyMaster, PtyPair, PtySlave},
    socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType},
    string::String,
    sync::Arc,
    task::{
        EXECVE_FORCE_ABI_REBUILD, WAIT_NOHANG, create_session, execve_with_flags, exit, fork,
        process_group_id, waitpid,
    },
    thread, vec,
    vec::Vec,
};
use subtle::ConstantTimeEq;
use sunset::{ChanData, ChanFail, ChanHandle, Event, Runner, ServEvent, Server, SignKey};

const DEFAULT_PORT: u16 = 22;
const ROOT_USER: &str = "root";
const AUTHORIZED_KEYS_PATH: &str = "/root/.ssh/authorized_keys";
const ED25519_HOST_KEY_PATH: &str = "/etc/ssh/ssh_host_ed25519_key";
const RSA_HOST_KEY_PATH: &str = "/etc/ssh/ssh_host_rsa_key";
const SSH_BUFFER_SIZE: usize = 8192;
const IO_BUFFER_SIZE: usize = 4096;
const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const PASSWORD_COST: u8 = 10;
const POST_EXIT_IDLE_LIMIT: u16 = 100;
const CUSTOM_RANDOM_ERROR: u32 = getrandom::Error::CUSTOM_START + 1;

getrandom::register_custom_getrandom!(scarlet_getrandom);

fn scarlet_getrandom(destination: &mut [u8]) -> Result<(), getrandom::Error> {
    let mut offset = 0usize;
    while offset < destination.len() {
        let result = scarlet_sys::syscall3(
            scarlet_sys::Syscall::GetRandom,
            destination[offset..].as_mut_ptr() as usize,
            destination.len() - offset,
            scarlet_sys::GET_RANDOM_FLAG_REQUIRE_ENTROPY,
        );
        if result == usize::MAX || result == 0 || result > destination.len() - offset {
            let code = NonZeroU32::new(CUSTOM_RANDOM_ERROR)
                .expect("custom getrandom error code must be non-zero");
            return Err(getrandom::Error::from(code));
        }
        offset += result;
    }
    Ok(())
}

#[derive(Clone)]
struct PasswordHash {
    salt: [u8; 16],
    hash: [u8; 24],
    cost: u8,
}

impl PasswordHash {
    fn new(password: &str) -> Result<Self, &'static str> {
        if password.is_empty() {
            return Err("the SSH password must not be empty");
        }

        let mut salt = [0u8; 16];
        getrandom::getrandom(&mut salt).map_err(|_| "failed to obtain password salt")?;
        let prehash = Self::prehash(password, &salt);
        let hash = bcrypt::bcrypt(PASSWORD_COST as u32, salt, &prehash);
        Ok(Self {
            salt,
            hash,
            cost: PASSWORD_COST,
        })
    }

    fn check(&self, password: &str) -> bool {
        if password.is_empty() {
            return false;
        }
        let prehash = Self::prehash(password, &self.salt);
        let candidate = bcrypt::bcrypt(self.cost as u32, self.salt, &prehash);
        candidate.ct_eq(&self.hash).into()
    }

    fn prehash(password: &str, salt: &[u8]) -> [u8; 32] {
        let mut prehash = Hmac::<Sha256>::new_from_slice(salt)
            .expect("HMAC-SHA256 accepts salts of every length");
        prehash.update(password.as_bytes());
        prehash.finalize().into_bytes().into()
    }
}

struct ServerConfig {
    host_keys: Vec<SignKey>,
    password: Option<PasswordHash>,
}

impl ServerConfig {
    fn load() -> Result<Self, String> {
        let password = match env::var("SCARLET_SSH_PASSWORD") {
            Some(password) if !password.is_empty() => {
                let hashed = PasswordHash::new(&password).map_err(String::from)?;
                env::remove_var("SCARLET_SSH_PASSWORD");
                Some(hashed)
            }
            Some(_) => {
                env::remove_var("SCARLET_SSH_PASSWORD");
                None
            }
            None => None,
        };

        let host_keys = load_host_keys()?;

        Ok(Self {
            host_keys,
            password,
        })
    }

    fn password_enabled(&self) -> bool {
        self.password.is_some()
    }
}

#[derive(Default)]
struct PendingBuffer {
    bytes: Vec<u8>,
    offset: usize,
}

impl PendingBuffer {
    fn is_empty(&self) -> bool {
        self.offset >= self.bytes.len()
    }

    fn remaining(&self) -> &[u8] {
        &self.bytes[self.offset..]
    }

    fn replace(&mut self, bytes: &[u8]) {
        self.bytes.clear();
        self.bytes.extend_from_slice(bytes);
        self.offset = 0;
    }

    fn consume(&mut self, count: usize) {
        self.offset += count;
        if self.offset >= self.bytes.len() {
            self.bytes.clear();
            self.offset = 0;
        }
    }
}

struct ChildSession {
    master: PtyMaster,
    pid: i32,
    to_pty: PendingBuffer,
    from_pty: PendingBuffer,
    child_exited: bool,
    pty_eof: bool,
    sent_veof: bool,
    post_exit_idle: u16,
    exit_status: u32,
    ssh_finished: bool,
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        if !self.child_exited {
            let _ = scarlet_sys::syscall2(scarlet_sys::Syscall::Kill, self.pid as usize, 15);
            let _ = waitpid(self.pid, WAIT_NOHANG);
        }
    }
}

struct TerminalSettings {
    term: Option<String>,
    columns: u16,
    rows: u16,
}

impl TerminalSettings {
    fn new() -> Self {
        Self {
            term: None,
            columns: DEFAULT_COLUMNS,
            rows: DEFAULT_ROWS,
        }
    }
}

struct ConnectionState {
    channel: Option<ChanHandle>,
    child: Option<ChildSession>,
    terminal: TerminalSettings,
    environment: Vec<(String, String)>,
    authorized_keys: Vec<[u8; 32]>,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            channel: None,
            child: None,
            terminal: TerminalSettings::new(),
            environment: Vec::new(),
            authorized_keys: load_authorized_keys(AUTHORIZED_KEYS_PATH),
        }
    }

    fn public_key_enabled(&self) -> bool {
        !self.authorized_keys.is_empty()
    }

    fn accepts_public_key(&self, key: &sunset::PubKey<'_>) -> bool {
        let Ok(fingerprint) = key.fingerprint(HashAlg::Sha256) else {
            return false;
        };
        let Some(candidate) = fingerprint.sha256() else {
            return false;
        };

        self.authorized_keys
            .iter()
            .any(|known| candidate.ct_eq(known).into())
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let config = match ServerConfig::load() {
        Ok(config) => Arc::new(config),
        Err(error) => {
            println!("[sshd] configuration failed: {}", error);
            return 1;
        }
    };

    let authorized_key_count = load_authorized_keys(AUTHORIZED_KEYS_PATH).len();
    if !config.password_enabled() && authorized_key_count == 0 {
        println!(
            "[sshd] warning: no authentication is configured; set SCARLET_SSH_PASSWORD or install {}",
            AUTHORIZED_KEYS_PATH
        );
    }

    let port = env::var("SCARLET_SSH_PORT")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);

    let listener =
        match Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
        {
            Ok(listener) => listener,
            Err(error) => {
                println!("[sshd] socket creation failed: {:?}", error);
                return 1;
            }
        };
    if let Err(error) = listener.bind_inet(Inet4SocketAddress::new([0, 0, 0, 0], port)) {
        println!("[sshd] bind 0.0.0.0:{} failed: {:?}", port, error);
        return 1;
    }
    if let Err(error) = listener.listen(16) {
        println!("[sshd] listen failed: {:?}", error);
        return 1;
    }

    println!(
        "[sshd] listening on 0.0.0.0:{} (password={}, authorized_keys={})",
        port,
        config.password_enabled(),
        authorized_key_count
    );

    loop {
        match listener.accept() {
            Ok(socket) => {
                let config = Arc::clone(&config);
                thread::spawn(move || {
                    println!("[sshd] client connected");
                    if let Err(error) = serve_connection(socket, &config) {
                        println!("[sshd] connection ended: {}", error);
                    } else {
                        println!("[sshd] client disconnected");
                    }
                });
            }
            Err(error) => {
                println!("[sshd] accept failed: {:?}", error);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn serve_connection(mut socket: Socket, config: &ServerConfig) -> Result<(), String> {
    socket
        .set_nonblocking(true)
        .map_err(|error| format!("failed to set socket nonblocking: {:?}", error))?;

    let socket_handle = socket.as_raw();
    let mut ssh_input = [0u8; SSH_BUFFER_SIZE];
    let mut ssh_output = [0u8; SSH_BUFFER_SIZE];
    let mut runner = Runner::new_server(&mut ssh_input, &mut ssh_output);
    let mut state = ConnectionState::new();
    let mut network_input = PendingBuffer::default();
    let mut socket_closed = false;
    let mut close_after_flush = false;

    loop {
        let mut progressed = false;

        if !network_input.is_empty() && runner.is_input_ready() {
            let count = runner
                .input(network_input.remaining())
                .map_err(|error| format!("SSH input failed: {:?}", error))?;
            if count > 0 {
                network_input.consume(count);
                progressed = true;
            }
        }

        if network_input.is_empty() && !socket_closed {
            let mut buffer = [0u8; IO_BUFFER_SIZE];
            match socket.read(&mut buffer) {
                Ok(0) => {
                    socket_closed = true;
                    runner.close_input();
                    progressed = true;
                }
                Ok(count) => {
                    network_input.replace(&buffer[..count]);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
                    socket_closed = true;
                    runner.close_input();
                    progressed = true;
                }
                Err(error) => return Err(format!("socket read failed: {}", error)),
            }
        }

        loop {
            match runner
                .progress()
                .map_err(|error| format!("SSH protocol failed: {:?}", error))?
            {
                Event::Progressed => progressed = true,
                Event::None => break,
                Event::Cli(_) => return Err(String::from("received a client event on a server")),
                Event::Serv(ServEvent::PollAgain) => break,
                Event::Serv(ServEvent::Defunct) => {
                    close_after_flush = true;
                    break;
                }
                Event::Serv(event) => {
                    handle_server_event(event, config, &mut state, socket_handle)?;
                    progressed = true;
                }
            }
        }

        if bridge_session(&mut runner, &mut state)? {
            progressed = true;
        }
        if session_finished(&state) && finish_ssh_session(&mut runner, &mut state)? {
            progressed = true;
        }
        if state.child.as_ref().is_some_and(|child| child.ssh_finished)
            && state
                .channel
                .as_ref()
                .is_some_and(|channel| runner.is_channel_closed(channel))
        {
            close_after_flush = true;
        }

        loop {
            let write_result = {
                let output = runner.output_buf();
                if output.is_empty() {
                    break;
                }
                socket.write(output)
            };
            match write_result {
                Ok(0) => return Err(String::from("socket returned a zero-length write")),
                Ok(count) => {
                    runner.consume_output(count);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(format!("socket write failed: {}", error)),
            }
        }

        if close_after_flush && !runner.is_output_pending() {
            return Ok(());
        }
        if socket_closed && network_input.is_empty() && !runner.is_output_pending() {
            return Ok(());
        }
        if !progressed {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn handle_server_event(
    event: ServEvent<'_, '_>,
    config: &ServerConfig,
    state: &mut ConnectionState,
    socket_handle: i32,
) -> Result<(), String> {
    match event {
        ServEvent::Hostkeys(request) => {
            let keys: Vec<&SignKey> = config.host_keys.iter().collect();
            request
                .hostkeys(&keys)
                .map_err(|error| format!("failed to provide host keys: {:?}", error))
        }
        ServEvent::FirstAuth(mut request) => {
            request
                .set_auth_methods(config.password_enabled(), state.public_key_enabled())
                .map_err(|error| format!("failed to configure authentication: {:?}", error))?;
            request
                .reject()
                .map_err(|error| format!("failed to continue authentication: {:?}", error))
        }
        ServEvent::PasswordAuth(request) => {
            let accepted = request.matches_username(ROOT_USER)
                && request
                    .password()
                    .ok()
                    .zip(config.password.as_ref())
                    .is_some_and(|(password, expected)| expected.check(password));
            if accepted {
                println!("[sshd] password authentication accepted for root");
                request
                    .allow()
                    .map_err(|error| format!("failed to accept password: {:?}", error))
            } else {
                request
                    .reject()
                    .map_err(|error| format!("failed to reject password: {:?}", error))
            }
        }
        ServEvent::PubkeyAuth(request) => {
            let accepted = request.username().ok().is_some_and(|username| {
                bool::from(username.as_bytes().ct_eq(ROOT_USER.as_bytes()))
            }) && request
                .pubkey()
                .is_ok_and(|key| state.accepts_public_key(&key));
            if accepted {
                if request.real() {
                    println!("[sshd] public-key authentication accepted for root");
                }
                request
                    .allow()
                    .map_err(|error| format!("failed to accept public key: {:?}", error))
            } else {
                request
                    .reject()
                    .map_err(|error| format!("failed to reject public key: {:?}", error))
            }
        }
        ServEvent::OpenSession(request) => {
            if state.channel.is_some() {
                request
                    .reject(ChanFail::SSH_OPEN_ADMINISTRATIVELY_PROHIBITED)
                    .map_err(|error| format!("failed to reject extra channel: {:?}", error))
            } else {
                let channel = request
                    .accept()
                    .map_err(|error| format!("failed to accept session channel: {:?}", error))?;
                state.channel = Some(channel);
                Ok(())
            }
        }
        ServEvent::SessionPty(request) => {
            let pty = request.pty();
            if channel_matches(state, request.channel())
                && state.child.is_none()
                && let Ok(pty) = pty
            {
                state.terminal.term = Some(String::from(pty.term.as_str()));
                state.terminal.columns = terminal_dimension(pty.cols, DEFAULT_COLUMNS);
                state.terminal.rows = terminal_dimension(pty.rows, DEFAULT_ROWS);
                request
                    .succeed()
                    .map_err(|error| format!("failed to accept PTY request: {:?}", error))
            } else {
                request
                    .fail()
                    .map_err(|error| format!("failed to reject PTY request: {:?}", error))
            }
        }
        ServEvent::SessionWindowChange(request) => {
            if !channel_matches(state, request.channel()) {
                return Ok(());
            }
            let (columns, rows, _, _) = request
                .dimensions()
                .map_err(|error| format!("invalid window-change request: {:?}", error))?;
            state.terminal.columns = terminal_dimension(columns, state.terminal.columns);
            state.terminal.rows = terminal_dimension(rows, state.terminal.rows);
            if let Some(child) = state.child.as_mut() {
                child
                    .master
                    .set_winsize(state.terminal.columns, state.terminal.rows)
                    .map_err(|error| format!("failed to resize PTY: {}", error))?;
            }
            Ok(())
        }
        ServEvent::SessionEnv(request) => {
            let name = request.name().map(String::from);
            let value = request.value().map(String::from);
            let accepted = match (name, value) {
                (Ok(name), Ok(value))
                    if channel_matches(state, request.channel())
                        && state.child.is_none()
                        && environment_allowed(&name, &value) =>
                {
                    state.environment.retain(|(existing, _)| existing != &name);
                    state.environment.push((name, value));
                    true
                }
                _ => false,
            };
            if accepted {
                request
                    .succeed()
                    .map_err(|error| format!("failed to accept environment: {:?}", error))
            } else {
                request
                    .fail()
                    .map_err(|error| format!("failed to reject environment: {:?}", error))
            }
        }
        ServEvent::SessionShell(request) => {
            if !channel_matches(state, request.channel()) || state.child.is_some() {
                return request
                    .fail()
                    .map_err(|error| format!("failed to reject shell request: {:?}", error));
            }
            match spawn_session(None, &state.environment, &state.terminal, socket_handle) {
                Ok(child) => {
                    state.child = Some(child);
                    request
                        .succeed()
                        .map_err(|error| format!("failed to accept shell request: {:?}", error))
                }
                Err(error) => {
                    println!("[sshd] shell spawn failed: {}", error);
                    request
                        .fail()
                        .map_err(|failure| format!("failed to reject shell request: {:?}", failure))
                }
            }
        }
        ServEvent::SessionExec(request) => {
            let command = request.command().map(String::from);
            if !channel_matches(state, request.channel()) || state.child.is_some() {
                return request
                    .fail()
                    .map_err(|error| format!("failed to reject exec request: {:?}", error));
            }
            let command = match command {
                Ok(command) if !command.is_empty() => command,
                _ => {
                    return request
                        .fail()
                        .map_err(|error| format!("failed to reject exec request: {:?}", error));
                }
            };
            match spawn_session(
                Some(command.as_str()),
                &state.environment,
                &state.terminal,
                socket_handle,
            ) {
                Ok(child) => {
                    state.child = Some(child);
                    request
                        .succeed()
                        .map_err(|error| format!("failed to accept exec request: {:?}", error))
                }
                Err(error) => {
                    println!("[sshd] command spawn failed: {}", error);
                    request
                        .fail()
                        .map_err(|failure| format!("failed to reject exec request: {:?}", failure))
                }
            }
        }
        ServEvent::SessionSubsystem(request) => request
            .fail()
            .map_err(|error| format!("failed to reject subsystem: {:?}", error)),
        ServEvent::Defunct | ServEvent::PollAgain => Ok(()),
    }
}

fn channel_matches(state: &ConnectionState, number: sunset::ChanNum) -> bool {
    state
        .channel
        .as_ref()
        .is_some_and(|channel| channel.num() == number)
}

fn terminal_dimension(value: u32, fallback: u16) -> u16 {
    if value == 0 {
        fallback
    } else {
        value.min(u16::MAX as u32) as u16
    }
}

fn environment_allowed(name: &str, value: &str) -> bool {
    if value.len() > 1024 || value.as_bytes().contains(&0) {
        return false;
    }
    let name_allowed = name == "TERM"
        || name == "LANG"
        || name
            .strip_prefix("LC_")
            .is_some_and(|suffix| !suffix.is_empty());
    name_allowed
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn spawn_session(
    command: Option<&str>,
    requested_environment: &[(String, String)],
    terminal: &TerminalSettings,
    socket_handle: i32,
) -> Result<ChildSession, &'static str> {
    let PtyPair {
        master,
        slave,
        slave_path: _,
    } = PtyPair::open().map_err(|_| "failed to allocate PTY")?;
    master
        .set_winsize(terminal.columns, terminal.rows)
        .map_err(|_| "failed to set PTY size")?;
    master
        .as_file()
        .set_nonblocking(true)
        .map_err(|_| "failed to set PTY nonblocking")?;

    let master_handle = master.as_file().as_raw();
    let environment = shell_environment(requested_environment, terminal.term.as_deref());
    let pid = fork();
    if pid == 0 {
        close_inherited_handle(master_handle);
        close_inherited_handle(socket_handle);
        let _ = create_session();
        setup_child_stdio(slave);

        let environment_refs: Vec<&str> = environment.iter().map(String::as_str).collect();
        let shell_candidates = [
            "/bin/sh",
            "/scarlet/system/scarlet/bin/sh",
            "/old_root/bin/sh",
        ];
        for shell in shell_candidates {
            let result = if let Some(command) = command {
                let arguments = [shell, "-c", command];
                execve_with_flags(
                    shell,
                    &arguments,
                    &environment_refs,
                    EXECVE_FORCE_ABI_REBUILD,
                )
            } else {
                let arguments = [shell];
                execve_with_flags(
                    shell,
                    &arguments,
                    &environment_refs,
                    EXECVE_FORCE_ABI_REBUILD,
                )
            };
            if result == 0 {
                break;
            }
        }
        println!("[sshd] failed to execute the system shell");
        exit(127);
    }
    if pid < 0 {
        return Err("fork failed");
    }

    drop(slave);
    Ok(ChildSession {
        master,
        pid,
        to_pty: PendingBuffer::default(),
        from_pty: PendingBuffer::default(),
        child_exited: false,
        pty_eof: false,
        sent_veof: false,
        post_exit_idle: 0,
        exit_status: 255,
        ssh_finished: false,
    })
}

fn shell_environment(requested: &[(String, String)], pty_term: Option<&str>) -> Vec<String> {
    let mut environment = vec![
        String::from("USER=root"),
        String::from("LOGNAME=root"),
        String::from("HOME=/root"),
        String::from("SHELL=/bin/sh"),
        String::from("PATH=/scarlet/system/scarlet/bin:/bin:/usr/bin"),
    ];
    let mut term = pty_term;

    for (name, value) in requested {
        if name == "TERM" {
            if term.is_none() {
                term = Some(value);
            }
            continue;
        }
        environment.push(format!("{}={}", name, value));
    }
    environment.push(format!("TERM={}", term.unwrap_or("xterm-256color")));
    environment
}

fn setup_child_stdio(slave: PtySlave) {
    let slave_handle = slave.into_file().into_handle();
    let terminal = std::tty::Terminal::from_handle(&slave_handle);
    let _ = terminal.acquire_as_controlling(false);
    if let Ok(group) = process_group_id(None) {
        let _ = terminal.set_foreground_group(group as usize);
    }
    duplicate_to_stdio(&slave_handle, 0);
    duplicate_to_stdio(&slave_handle, 1);
    duplicate_to_stdio(&slave_handle, 2);
}

fn duplicate_to_stdio(source: &Handle, target: i32) {
    if let Ok(handle) = unsafe { Handle::from_raw(target) } {
        let _ = handle.close();
    }
    match source.duplicate() {
        Ok(handle) if handle.as_raw() == target => core::mem::forget(handle),
        Ok(handle) => {
            println!(
                "[sshd] duplicated stdio handle {}, expected {}",
                handle.as_raw(),
                target
            );
            core::mem::forget(handle);
        }
        Err(error) => println!("[sshd] failed to duplicate fd {}: {:?}", target, error),
    }
}

fn close_inherited_handle(raw_handle: i32) {
    if raw_handle >= 0
        && let Ok(handle) = unsafe { Handle::from_raw(raw_handle) }
    {
        drop(handle);
    }
}

fn bridge_session(
    runner: &mut Runner<'_, Server>,
    state: &mut ConnectionState,
) -> Result<bool, String> {
    let Some(channel) = state.channel.as_ref() else {
        return Ok(false);
    };
    let Some(child) = state.child.as_mut() else {
        return Ok(false);
    };
    let mut progressed = false;

    if !child.to_pty.is_empty() {
        match child.master.write(child.to_pty.remaining()) {
            Ok(0) => {}
            Ok(count) => {
                child.to_pty.consume(count);
                progressed = true;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("PTY write failed: {}", error)),
        }
    }

    if child.to_pty.is_empty()
        && let Some((number, ChanData::Normal, ready)) = runner.read_channel_ready()
        && number == channel.num()
    {
        let mut buffer = [0u8; IO_BUFFER_SIZE];
        let limit = ready.min(buffer.len());
        let count = runner
            .read_channel(channel, ChanData::Normal, &mut buffer[..limit])
            .map_err(|error| format!("SSH channel read failed: {:?}", error))?;
        if count > 0 {
            child.to_pty.replace(&buffer[..count]);
            progressed = true;
        }
    }

    if runner.is_channel_eof(channel) && !child.sent_veof {
        if child.to_pty.is_empty() {
            child.to_pty.replace(&[0x04]);
        } else {
            child.to_pty.bytes.push(0x04);
        }
        child.sent_veof = true;
        progressed = true;
    }

    if !child.from_pty.is_empty() {
        let count = runner
            .write_channel(channel, ChanData::Normal, child.from_pty.remaining())
            .map_err(|error| format!("SSH channel write failed: {:?}", error))?;
        if count > 0 {
            child.from_pty.consume(count);
            child.post_exit_idle = 0;
            progressed = true;
        }
    }

    if child.from_pty.is_empty() && !child.pty_eof {
        let ready = runner
            .write_channel_ready(channel, ChanData::Normal)
            .map_err(|error| format!("SSH channel readiness failed: {:?}", error))?
            .unwrap_or(0);
        if ready > 0 {
            let mut buffer = [0u8; IO_BUFFER_SIZE];
            let limit = ready.min(buffer.len());
            match child.master.read(&mut buffer[..limit]) {
                Ok(0) => {
                    child.pty_eof = true;
                    progressed = true;
                }
                Ok(count) => {
                    child.from_pty.replace(&buffer[..count]);
                    child.post_exit_idle = 0;
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if child.child_exited => {
                    child.pty_eof = true;
                    println!("[sshd] PTY closed after child exit: {}", error);
                    progressed = true;
                }
                Err(error) => return Err(format!("PTY read failed: {}", error)),
            }
        }
    }

    if !child.child_exited {
        let (pid, status) = waitpid(child.pid, WAIT_NOHANG);
        if pid == child.pid {
            child.child_exited = true;
            child.exit_status = if status < 0 { 255 } else { status as u32 };
            child.post_exit_idle = 0;
            progressed = true;
        } else if pid < 0 {
            child.child_exited = true;
            child.exit_status = 255;
            child.post_exit_idle = 0;
            progressed = true;
        }
    } else if child.from_pty.is_empty() {
        child.post_exit_idle = child.post_exit_idle.saturating_add(1);
    }

    Ok(progressed)
}

fn session_finished(state: &ConnectionState) -> bool {
    state.child.as_ref().is_some_and(|child| {
        child.child_exited
            && child.from_pty.is_empty()
            && (child.pty_eof || child.post_exit_idle >= POST_EXIT_IDLE_LIMIT)
    })
}

fn finish_ssh_session(
    runner: &mut Runner<'_, Server>,
    state: &mut ConnectionState,
) -> Result<bool, String> {
    let Some(channel) = state.channel.as_ref() else {
        return Ok(false);
    };
    let Some(child) = state.child.as_mut() else {
        return Ok(false);
    };
    if child.ssh_finished {
        return Ok(false);
    }

    runner
        .send_channel_exit_status(channel, child.exit_status)
        .map_err(|error| format!("failed to send SSH exit status: {:?}", error))?;
    runner
        .send_channel_eof(channel)
        .map_err(|error| format!("failed to send SSH channel EOF: {:?}", error))?;
    runner
        .send_channel_close(channel)
        .map_err(|error| format!("failed to close SSH channel: {:?}", error))?;
    child.ssh_finished = true;
    Ok(true)
}

fn load_host_keys() -> Result<Vec<SignKey>, String> {
    let mut keys = Vec::new();
    if let Some(key) = load_host_key(ED25519_HOST_KEY_PATH) {
        println!("[sshd] loaded {}", ED25519_HOST_KEY_PATH);
        keys.push(key);
    } else {
        println!("[sshd] generating a new Ed25519 host key");
        keys.push(generate_host_key(
            ED25519_HOST_KEY_PATH,
            Algorithm::Ed25519,
        )?);
    }

    if let Some(key) = load_host_key(RSA_HOST_KEY_PATH) {
        println!("[sshd] loaded {}", RSA_HOST_KEY_PATH);
        keys.push(key);
    } else {
        println!("[sshd] generating a new RSA host key");
        keys.push(generate_host_key(
            RSA_HOST_KEY_PATH,
            Algorithm::Rsa { hash: None },
        )?);
    }

    for key in &keys {
        if let Ok(fingerprint) = key.pubkey().fingerprint(HashAlg::Sha256) {
            println!("[sshd] host key {}", fingerprint);
        }
    }
    Ok(keys)
}

fn generate_host_key(path: &str, algorithm: Algorithm) -> Result<SignKey, String> {
    let mut private_key = PrivateKey::random(&mut OsRng, algorithm)
        .map_err(|error| format!("host-key generation failed: {:?}", error))?;
    private_key.set_comment("scarlet-sshd");
    let encoded = private_key
        .to_openssh(LineEnding::LF)
        .map_err(|error| format!("host-key encoding failed: {:?}", error))?;
    let signing_key = SignKey::from_openssh(encoded.as_bytes())
        .map_err(|error| format!("generated host key is unsupported: {:?}", error))?;

    let _ = create_directory("/etc/ssh");
    let persisted = File::create(path).and_then(|mut file| file.write_all(encoded.as_bytes()));
    match persisted {
        Ok(()) => println!("[sshd] saved {}", path),
        Err(error) => println!(
            "[sshd] warning: could not persist {}: {}; this key is ephemeral",
            path, error
        ),
    }

    Ok(signing_key)
}

fn load_host_key(path: &str) -> Option<SignKey> {
    let bytes = read_file(path)?;
    match SignKey::from_openssh(&bytes) {
        Ok(key) => Some(key),
        Err(error) => {
            println!("[sshd] ignored invalid host key {}: {:?}", path, error);
            None
        }
    }
}

fn load_authorized_keys(path: &str) -> Vec<[u8; 32]> {
    let Some(bytes) = read_file(path) else {
        println!("[sshd] {} is not present", path);
        return Vec::new();
    };
    let Ok(contents) = core::str::from_utf8(&bytes) else {
        println!("[sshd] {} is not UTF-8", path);
        return Vec::new();
    };

    let mut fingerprints = Vec::new();
    for (line_number, line) in contents.lines().enumerate() {
        let Some(key_text) = authorized_key_text(line) else {
            continue;
        };
        match PublicKey::from_openssh(&key_text) {
            Ok(key) => {
                if let Some(fingerprint) = key.fingerprint(HashAlg::Sha256).sha256()
                    && !fingerprints.contains(&fingerprint)
                {
                    fingerprints.push(fingerprint);
                }
            }
            Err(error) => println!(
                "[sshd] ignored {} line {}: {:?}",
                path,
                line_number + 1,
                error
            ),
        }
    }
    println!("[sshd] loaded {} authorized key(s)", fingerprints.len());
    fingerprints
}

fn authorized_key_text(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    // Scarlet does not yet enforce authorized_keys options. Reject those
    // lines instead of silently dropping restrictions such as `from=`.
    let mut tokens = trimmed.split_ascii_whitespace();
    let algorithm = tokens.next()?;
    if algorithm == "ssh-ed25519" || algorithm == "ssh-rsa" {
        let encoded = tokens.next()?;
        return Some(format!("{} {}", algorithm, encoded));
    }
    None
}

fn read_file(path: &str) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut contents = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Some(contents),
            Ok(count) => {
                if contents.len().saturating_add(count) > 256 * 1024 {
                    println!("[sshd] refused oversized file {}", path);
                    return None;
                }
                contents.extend_from_slice(&buffer[..count]);
            }
            Err(error) => {
                println!("[sshd] failed to read {}: {}", path, error);
                return None;
            }
        }
    }
}
