//! Pure-Rust SSH server for Scarlet.
//!
//! The transport and cryptography are provided by Sunset. Scarlet's native
//! `GetRandom` syscall backs the Rust `getrandom` ecosystem, and authenticated
//! shell and SFTP workers run with the credentials of the authenticated account.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

mod host_keys;
mod sftp;

use core::{num::NonZeroU32, time::Duration};

use host_keys::load_host_keys;
use scarlet_accounts::{Account, authenticate_local, authorized_keys, lookup_local};
use scarlet_os::process_scope::ProcessScope;
use ssh_key::{HashAlg, PublicKey};
use std::{
    env, format,
    fs::File,
    handle::Handle,
    io::{ErrorKind, Read, Write},
    println,
    pty::{PtyMaster, PtyPair, PtySlave},
    socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType},
    string::String,
    sync::Arc,
    task::{WAIT_NOHANG, create_session, execve, exit, fork, process_group_id, waitpid},
    thread, vec,
    vec::Vec,
};
use subtle::ConstantTimeEq;
use sunset::{ChanData, ChanFail, ChanHandle, Event, Runner, ServEvent, Server, SignKey};

const DEFAULT_PORT: u16 = 22;
const SSH_BUFFER_SIZE: usize = 64 * 1024;
const SSH_RECEIVE_WINDOW: u32 = 1024 * 1024;
const SSH_MAX_CHANNEL_PACKET: u32 = 32 * 1024;
const SFTP_READ_CHUNK: usize = 32 * 1024;
const IO_BUFFER_SIZE: usize = 4096;
const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const POST_EXIT_IDLE_LIMIT: u16 = 100;
const CUSTOM_RANDOM_ERROR: u32 = getrandom::Error::CUSTOM_START + 1;

getrandom::register_custom_getrandom!(scarlet_getrandom);

fn scarlet_getrandom(destination: &mut [u8]) -> Result<(), getrandom::Error> {
    let mut offset = 0usize;
    while offset < destination.len() {
        // SAFETY: The remaining destination slice is exclusively borrowed and writable for exactly the supplied length until return.
        let result = unsafe {
            scarlet_sys::syscall3(
                scarlet_sys::Syscall::GetRandom,
                destination[offset..].as_mut_ptr() as usize,
                destination.len() - offset,
                scarlet_sys::GET_RANDOM_FLAG_REQUIRE_ENTROPY,
            )
        };
        if result == usize::MAX || result == 0 || result > destination.len() - offset {
            let code = NonZeroU32::new(CUSTOM_RANDOM_ERROR)
                .expect("custom getrandom error code must be non-zero");
            return Err(getrandom::Error::from(code));
        }
        offset += result;
    }
    Ok(())
}

struct ServerConfig {
    host_keys: Vec<SignKey>,
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
    io: SessionIo,
    scope: ProcessScope,
    pid: i32,
    to_child: PendingBuffer,
    from_child: PendingBuffer,
    child_exited: bool,
    output_eof: bool,
    input_closed: bool,
    post_exit_idle: u16,
    exit_status: u32,
    ssh_finished: bool,
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        let _ = self.scope.terminate();
        if !self.child_exited {
            let _ = waitpid(self.pid, 0);
        }
    }
}

enum SessionIo {
    Terminal(PtyMaster),
    Binary(Socket),
}

enum ChildIo {
    Terminal(PtySlave),
    Binary(Socket),
}

enum SessionKind<'a> {
    Shell,
    Command(&'a str),
    Sftp,
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
    account: Option<Account>,
    terminal: TerminalSettings,
    environment: Vec<(String, String)>,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            channel: None,
            child: None,
            account: None,
            terminal: TerminalSettings::new(),
            environment: Vec::new(),
        }
    }
}
fn accepts_public_key(account: &Account, key: &sunset::PubKey<'_>) -> bool {
    let Some(candidate) = key
        .fingerprint(HashAlg::Sha256)
        .ok()
        .and_then(|key| key.sha256())
    else {
        return false;
    };
    authorized_keys(account).is_some_and(|bytes| {
        parse_authorized_keys(&bytes)
            .iter()
            .any(|known| bool::from(candidate.ct_eq(known)))
    })
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    if env::args_vec().get(1).map(String::as_str) == Some("--sftp-worker") {
        return if run_sftp_worker().is_ok() { 0 } else { 1 };
    }
    let config = match load_host_keys() {
        Ok(host_keys) => Arc::new(ServerConfig { host_keys }),
        Err(error) => {
            println!("[sshd] configuration failed: {}", error);
            return 1;
        }
    };

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

    println!("[sshd] listening on 0.0.0.0:{}", port);

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

    let mut ssh_input = vec![0u8; SSH_BUFFER_SIZE];
    let mut ssh_output = vec![0u8; SSH_BUFFER_SIZE];
    let mut runner = Runner::new_server_with_channel_limits(
        &mut ssh_input,
        &mut ssh_output,
        SSH_RECEIVE_WINDOW,
        SSH_MAX_CHANNEL_PACKET,
    )
    .map_err(|error| format!("SSH buffer configuration failed: {:?}", error))?;
    let mut network_read = vec![0u8; SFTP_READ_CHUNK];
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
            match socket.read(&mut network_read) {
                Ok(0) => {
                    socket_closed = true;
                    runner.close_input();
                    progressed = true;
                }
                Ok(count) => {
                    network_input.replace(&network_read[..count]);
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
                    handle_server_event(event, config, &mut state)?;
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
) -> Result<(), String> {
    match event {
        ServEvent::Hostkeys(request) => {
            let keys: Vec<&SignKey> = config.host_keys.iter().collect();
            request
                .hostkeys(&keys)
                .map_err(|error| format!("failed to provide host keys: {:?}", error))
        }
        ServEvent::FirstAuth(mut request) => {
            let allow_password = request.username().is_ok_and(|name| name != "root");
            request
                .set_auth_methods(allow_password, true)
                .map_err(|error| format!("failed to configure authentication: {:?}", error))?;
            request
                .reject()
                .map_err(|error| format!("failed to continue authentication: {:?}", error))
        }
        ServEvent::PasswordAuth(request) => {
            state.account = match (request.username(), request.password()) {
                (Ok(name), Ok(password)) if name != "root" => {
                    authenticate_local(name.as_bytes(), password.as_bytes())
                        .ok()
                        .flatten()
                }
                _ => None,
            };
            if state.account.is_some() {
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
            let account = request
                .username()
                .ok()
                .and_then(|name| lookup_local(name.as_bytes()).ok().flatten())
                .filter(|account| {
                    request
                        .pubkey()
                        .is_ok_and(|key| accepts_public_key(account, &key))
                });
            if account.is_some() {
                if request.real() {
                    state.account = account;
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
            if state.account.is_none() || state.channel.is_some() {
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
            if let Some(ChildSession {
                io: SessionIo::Terminal(master),
                ..
            }) = state.child.as_mut()
            {
                master
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
            match spawn_session(SessionKind::Shell, state) {
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
            match spawn_session(SessionKind::Command(&command), state) {
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
        ServEvent::SessionSubsystem(request) => {
            if !channel_matches(state, request.channel())
                || state.child.is_some()
                || request.command().ok() != Some("sftp")
            {
                return request
                    .fail()
                    .map_err(|error| format!("failed to reject subsystem: {:?}", error));
            }
            match spawn_session(SessionKind::Sftp, state) {
                Ok(child) => {
                    state.child = Some(child);
                    request
                        .succeed()
                        .map_err(|error| format!("failed to accept SFTP: {:?}", error))
                }
                Err(_) => request
                    .fail()
                    .map_err(|e| format!("failed to reject SFTP: {:?}", e)),
            }
        }
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
    kind: SessionKind<'_>,
    state: &ConnectionState,
) -> Result<ChildSession, &'static str> {
    let account = state.account.as_ref().ok_or("unauthenticated session")?;
    let scope = ProcessScope::create().map_err(|_| "cannot supervise SSH session")?;
    let (io, child_io) = match kind {
        SessionKind::Sftp => {
            let (parent, child) = Socket::pair().map_err(|_| "cannot create SFTP channel")?;
            parent
                .set_nonblocking(true)
                .map_err(|_| "cannot configure SFTP channel")?;
            (SessionIo::Binary(parent), ChildIo::Binary(child))
        }
        _ => {
            let PtyPair { master, slave, .. } =
                PtyPair::open().map_err(|_| "cannot allocate PTY")?;
            master
                .set_winsize(state.terminal.columns, state.terminal.rows)
                .map_err(|_| "cannot size PTY")?;
            master
                .as_file()
                .set_nonblocking(true)
                .map_err(|_| "cannot configure PTY")?;
            (SessionIo::Terminal(master), ChildIo::Terminal(slave))
        }
    };
    let environment =
        shell_environment(account, &state.environment, state.terminal.term.as_deref());
    let environment_refs: Vec<&str> = environment.iter().map(String::as_str).collect();
    let home = account.home();
    let pid = fork();
    if pid == 0 {
        drop(io);
        match child_io {
            ChildIo::Terminal(slave) => {
                if create_session().is_err() {
                    exit(126);
                }
                setup_child_stdio(slave);
            }
            ChildIo::Binary(socket) => {
                let stream = socket.into_handle();
                let Ok(null) = File::open_with_flags("/dev/null", 1) else {
                    exit(126)
                };
                duplicate_to_stdio(&stream, 0);
                duplicate_to_stdio(&stream, 1);
                duplicate_to_stdio(&null.into_handle(), 2);
            }
        }
        // SAFETY: this exclusive fork child proceeds directly to exec or exit.
        // No inherited owners of revoked descriptors are subsequently dropped.
        if unsafe { account.enter(scope) }.is_err() || std::fs::change_directory(&home).is_err() {
            exit(126);
        }
        match kind {
            SessionKind::Shell => execve("/bin/sh", &["-sh"], &environment_refs),
            SessionKind::Command(command) => {
                execve("/bin/sh", &["sh", "-c", command], &environment_refs)
            }
            SessionKind::Sftp => execve("/bin/sshd", &["sshd", "--sftp-worker"], &environment_refs),
        };
        exit(127);
    }
    if pid < 0 {
        return Err("fork failed");
    }

    drop(child_io);
    Ok(ChildSession {
        io,
        scope,
        pid,
        to_child: PendingBuffer::default(),
        from_child: PendingBuffer::default(),
        child_exited: false,
        output_eof: false,
        input_closed: false,
        post_exit_idle: 0,
        exit_status: 255,
        ssh_finished: false,
    })
}

fn shell_environment(
    account: &Account,
    requested: &[(String, String)],
    pty_term: Option<&str>,
) -> Vec<String> {
    let mut environment = vec![
        format!("USER={}", account.name),
        format!("LOGNAME={}", account.name),
        format!("HOME={}", account.home()),
        String::from("SHELL=/bin/sh"),
        String::from("PATH=/bin:/usr/bin"),
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
        _ => exit(126),
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

    if !child.to_child.is_empty() {
        let written = match &mut child.io {
            SessionIo::Terminal(master) => master.write(child.to_child.remaining()),
            SessionIo::Binary(stream) => stream.write(child.to_child.remaining()),
        };
        match written {
            Ok(0) => {}
            Ok(count) => {
                child.to_child.consume(count);
                progressed = true;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("child input write failed: {}", error)),
        }
    }

    if child.to_child.is_empty()
        && let Some((number, ChanData::Normal, ready)) = runner.read_channel_ready()
        && number == channel.num()
    {
        let mut buffer = [0u8; IO_BUFFER_SIZE];
        let limit = ready.min(buffer.len());
        let count = runner
            .read_channel(channel, ChanData::Normal, &mut buffer[..limit])
            .map_err(|error| format!("SSH channel read failed: {:?}", error))?;
        if count > 0 {
            child.to_child.replace(&buffer[..count]);
            progressed = true;
        }
    }

    if runner.is_channel_eof(channel)
        && !child.input_closed
        && child.to_child.is_empty()
        && runner.read_channel_ready().is_none()
    {
        match &child.io {
            SessionIo::Terminal(_) => child.to_child.replace(&[0x04]),
            SessionIo::Binary(stream) => stream
                .shutdown(std::socket::ShutdownHow::Write)
                .map_err(|_| "SFTP shutdown failed")?,
        }
        child.input_closed = true;
        progressed = true;
    }

    if !child.from_child.is_empty() {
        let count = runner
            .write_channel(channel, ChanData::Normal, child.from_child.remaining())
            .map_err(|error| format!("SSH channel write failed: {:?}", error))?;
        if count > 0 {
            child.from_child.consume(count);
            child.post_exit_idle = 0;
            progressed = true;
        }
    }

    if child.from_child.is_empty() && !child.output_eof {
        let ready = runner
            .write_channel_ready(channel, ChanData::Normal)
            .map_err(|error| format!("SSH channel readiness failed: {:?}", error))?
            .unwrap_or(0);
        if ready > 0 {
            let mut buffer = [0u8; IO_BUFFER_SIZE];
            let limit = ready.min(buffer.len());
            let read = match &mut child.io {
                SessionIo::Terminal(master) => master.read(&mut buffer[..limit]),
                SessionIo::Binary(stream) => stream.read(&mut buffer[..limit]),
            };
            match read {
                Ok(0) => {
                    child.output_eof = true;
                    progressed = true;
                }
                Ok(count) => {
                    child.from_child.replace(&buffer[..count]);
                    child.post_exit_idle = 0;
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == ErrorKind::UnexpectedEof || child.child_exited => {
                    child.output_eof = true;
                    progressed = true;
                }
                Err(error) => return Err(format!("child output read failed: {}", error)),
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
    } else if child.from_child.is_empty() {
        child.post_exit_idle = child.post_exit_idle.saturating_add(1);
    }

    Ok(progressed)
}

/// Invoked after credentials and stdio are installed and the SSH transport image has been replaced.
fn run_sftp_worker() -> Result<(), &'static str> {
    let home = env::var("HOME").ok_or("missing session home")?;
    let mut server = sftp::Server::new(&home);
    let mut chunk = vec![0; SFTP_READ_CHUNK];
    loop {
        while let Some(reply) = server.next_response()? {
            std::io::stdout()
                .write_all(&reply)
                .map_err(|_| "SFTP output failed")?;
        }
        let n = std::io::stdin()
            .read(&mut chunk)
            .map_err(|_| "SFTP input failed")?;
        if n == 0 {
            return if server.has_partial_request() {
                Err("truncated SFTP request")
            } else {
                Ok(())
            };
        }
        server.feed(&chunk[..n])?;
    }
}

fn session_finished(state: &ConnectionState) -> bool {
    state.child.as_ref().is_some_and(|child| {
        child.child_exited
            && child.from_child.is_empty()
            && (child.output_eof
                || (matches!(child.io, SessionIo::Terminal(_))
                    && child.post_exit_idle >= POST_EXIT_IDLE_LIMIT))
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

fn parse_authorized_keys(bytes: &[u8]) -> Vec<[u8; 32]> {
    let Ok(contents) = core::str::from_utf8(bytes) else {
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
                "[sshd] ignored authorized_keys line {}: {:?}",
                line_number + 1,
                error
            ),
        }
    }
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
