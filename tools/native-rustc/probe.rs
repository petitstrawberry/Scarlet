//! Execute on Scarlet; a full PASS requires compiling and running a new program here.
use std::cell::Cell;
use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HELLO: &str = "SCARLET_NATIVE_RUSTC_HELLO_OK\n";
const HELLO_EXIT: i32 = 37;
const CARGO_HELLO: &str = "SCARLET_NATIVE_CARGO_HELLO_OK\n";
const CARGO_ONLINE_HELLO: &str = "SCARLET_NATIVE_CARGO_ONLINE_OK=42\n";
const MACRO_HELLO: &str = "SCARLET_NATIVE_PROC_MACRO_OK=42\n";
const ZLIB_HELLO: &str = "SCARLET_LIBC_ZLIB_OK";
const SQLITE_HELLO: &str = "SCARLET_LIBC_SQLITE_OK";
const SQLITE_PTHREAD_HELLO: &str = "SCARLET_LIBC_SQLITE_PTHREAD_OK";
const SQLITE_CRASH_READY: &str = "SCARLET_LIBC_SQLITE_CRASH_READY";
const SQLITE_JOURNAL_MAGIC: &[u8] = &[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];
const CONFIG: &str = "/etc/native-rustc-probe.args";
#[cfg(all(feature = "native-fs", target_os = "scarlet"))]
mod allocation_failure;
#[cfg(all(feature = "native-fs", target_os = "scarlet"))]
mod native_fs;
const USAGE: &str = "usage: native-rustc-probe RUSTC SYSROOT TARGET NEW_OUTPUT_DIR [--dummy] [--full --linker PATH] [--cargo PATH [--cargo-online --resolverd PATH]] [--proc-macro] [--native-fs] [--c-startup PATH] [--zlib PATH] [--sqlite PATH] [--backend PATH_OR_NAME] [--linker-flavor FLAVOR] [--timeout SECONDS] [--run-timeout SECONDS]; no arguments reads /etc/native-rustc-probe.args (one argument per line)";

thread_local! {
    static THREAD_PREFLIGHT: Cell<u32> = const { Cell::new(0) };
}

#[derive(Debug)]
struct Options {
    rustc: PathBuf,
    sysroot: PathBuf,
    target: String,
    output: PathBuf,
    full: bool,
    dummy: bool,
    proc_macro: bool,
    native_fs: bool,
    cargo: Option<PathBuf>,
    cargo_online: bool,
    resolverd: Option<PathBuf>,
    c_startup: Option<PathBuf>,
    zlib: Option<PathBuf>,
    sqlite: Option<PathBuf>,
    backend: Option<String>,
    linker: Option<PathBuf>,
    linker_flavor: Option<String>,
    timeout: Duration,
    run_timeout: Duration,
}

fn seconds(value: &str) -> Result<Duration, String> {
    let seconds: u64 = value
        .parse()
        .map_err(|_| "timeout must be an integer".to_string())?;
    if !(1..=86400).contains(&seconds) {
        return Err("timeout must be between 1 and 86400 seconds".into());
    }
    Ok(Duration::from_secs(seconds))
}

fn options(args: &[String]) -> Result<Options, String> {
    if args.len() < 4 {
        return Err(USAGE.into());
    }
    let mut result = Options {
        rustc: PathBuf::from(&args[0]),
        sysroot: PathBuf::from(&args[1]),
        target: args[2].clone(),
        output: PathBuf::from(&args[3]),
        full: false,
        dummy: false,
        proc_macro: false,
        native_fs: false,
        cargo: None,
        cargo_online: false,
        resolverd: None,
        c_startup: None,
        zlib: None,
        sqlite: None,
        backend: None,
        linker: None,
        linker_flavor: None,
        timeout: Duration::from_secs(900),
        run_timeout: Duration::from_secs(60),
    };
    let mut rest = args[4..].iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--full" => result.full = true,
            "--dummy" => result.dummy = true,
            "--proc-macro" => result.proc_macro = true,
            "--native-fs" => result.native_fs = true,
            "--cargo-online" => result.cargo_online = true,
            "--backend" | "--linker" | "--linker-flavor" | "--timeout" | "--run-timeout"
            | "--c-startup" | "--zlib" | "--sqlite" | "--cargo" | "--resolverd" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                if value.is_empty() {
                    return Err(format!("empty value for {flag}"));
                }
                match flag.as_str() {
                    "--backend" => result.backend = Some(value.clone()),
                    "--c-startup" => result.c_startup = Some(value.into()),
                    "--zlib" => result.zlib = Some(value.into()),
                    "--sqlite" => result.sqlite = Some(value.into()),
                    "--cargo" => result.cargo = Some(value.into()),
                    "--resolverd" => result.resolverd = Some(value.into()),
                    "--linker" => result.linker = Some(value.into()),
                    "--linker-flavor" => result.linker_flavor = Some(value.clone()),
                    "--timeout" => result.timeout = seconds(value)?,
                    "--run-timeout" => result.run_timeout = seconds(value)?,
                    _ => unreachable!(),
                }
            }
            _ => return Err(format!("unknown option {flag}; {USAGE}")),
        }
    }
    if result.full && result.dummy {
        return Err("--dummy is only a frontend diagnostic and cannot be used with --full".into());
    }
    if result.proc_macro && !result.full {
        return Err("--proc-macro requires --full".into());
    }
    if result.cargo.is_some() && !result.full {
        return Err("--cargo requires --full".into());
    }
    if result.cargo_online && (result.cargo.is_none() || result.resolverd.is_none()) {
        return Err("--cargo-online requires --cargo and --resolverd".into());
    }
    if result.resolverd.is_some() && !result.cargo_online {
        return Err("--resolverd requires --cargo-online".into());
    }
    if result.dummy && result.backend.is_some() {
        return Err("--dummy and --backend are mutually exclusive".into());
    }
    if result.full && result.linker.is_none() {
        return Err("--full requires --linker pointing to a native Scarlet executable".into());
    }
    if !result.full && (result.linker.is_some() || result.linker_flavor.is_some()) {
        return Err("linker options require --full".into());
    }
    if !matches!(
        result.target.as_str(),
        "riscv64gc-unknown-scarlet" | "aarch64-unknown-scarlet"
    ) {
        return Err("expected a supported 64-bit native Scarlet target".into());
    }
    Ok(result)
}

fn phase(
    mut command: Command,
    output: &Path,
    name: &str,
    timeout: Duration,
    expected_exit: i32,
) -> Result<Vec<u8>, String> {
    let stdout_path = output.join(format!("{name}.stdout"));
    let stderr_path = output.join(format!("{name}.stderr"));
    let stdout = File::create(&stdout_path).map_err(|e| e.to_string())?;
    let stderr = File::create(&stderr_path).map_err(|e| e.to_string())?;
    command
        .current_dir(output)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let invocation = format!("{command:?}");
    fs::write(
        output.join(format!("{name}.command")),
        format!("{invocation}\n"),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "NATIVE_RUSTC START {name} timeout={} command={invocation}",
        timeout.as_secs()
    );
    let mut child = command.spawn().map_err(|e| format!("{name}: spawn: {e}"))?;
    // Release the parent copies of redirected descriptors. Ext2 persists a file
    // when its final handle closes; child exit must finish that before we read.
    drop(command);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => (),
            Err(e) => return Err(format!("{name}: wait: {e}")),
        }
        if started.elapsed() > timeout {
            // A timed-out child may still own the output descriptors on Scarlet.
            // Read the in-flight log before leaving the guest so the host-side
            // image extraction does not depend on the child's final close.
            if let Ok(bytes) = fs::read(&stderr_path) {
                let tail = &bytes[bytes.len().saturating_sub(12 * 1024)..];
                eprintln!(
                    "NATIVE_RUSTC TIMEOUT {name} stderr_tail_bytes={}:\n{}",
                    tail.len(),
                    String::from_utf8_lossy(tail)
                );
            }
            // Scarlet currently does not support Child::kill. Never block in
            // wait() after a failed kill: the host's QEMU deadline reaps the VM.
            let killed = child.kill();
            if killed.is_ok() {
                let _ = child.wait();
            }
            return Err(format!(
                "{name}: timed out after {} seconds; kill result: {killed:?}",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let out = fs::read(stdout_path).map_err(|e| e.to_string())?;
    let err = fs::read(stderr_path).map_err(|e| e.to_string())?;
    println!(
        "NATIVE_RUSTC OUTPUT {name} stdout_bytes={} stderr_bytes={}",
        out.len(),
        err.len()
    );
    print!("{}", String::from_utf8_lossy(&out));
    eprint!("{}", String::from_utf8_lossy(&err));
    let outcome = format!(
        "exit={:?} elapsed_ms={} expected_exit={expected_exit}\n",
        status.code(),
        started.elapsed().as_millis()
    );
    fs::write(output.join(format!("{name}.status")), &outcome).map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC STATUS {name} {}", outcome.trim_end());
    if status.code() != Some(expected_exit) {
        if status.code() == Some(139) {
            // A single read captures the current ring without waiting for EOF
            // on /dev/kmsg, which remains open for future kernel messages.
            use std::io::Read;
            let mut log = vec![0; 1024 * 1024];
            if let Ok(size) = File::open("/dev/kmsg").and_then(|mut file| file.read(&mut log)) {
                let _ = fs::write(output.join(format!("{name}.kernel.log")), &log[..size]);
            }
        }
        return Err(format!(
            "{name}: expected exit {expected_exit}, got {status}"
        ));
    }
    Ok(out)
}

fn compiler(options: &Options) -> Command {
    let mut command = Command::new(&options.rustc);
    command.arg("--sysroot").arg(&options.sysroot);
    if options.dummy {
        command.arg("-Zcodegen-backend=dummy");
    } else if let Some(backend) = &options.backend {
        command.arg(format!("-Zcodegen-backend={backend}"));
    }
    command
}

fn native_compiler(options: &Options) -> Command {
    let mut command = compiler(options);
    command.args([
        "--target",
        &options.target,
        "--edition=2021",
        "-Cpanic=abort",
    ]);
    command.arg(format!(
        "-Clinker={}",
        options.linker.as_ref().unwrap().display()
    ));
    if let Some(flavor) = &options.linker_flavor {
        command.arg(format!("-Clinker-flavor={flavor}"));
    }
    command
}

fn check_proc_macro(options: &Options) -> Result<(), String> {
    let output = &options.output;
    fs::write(output.join("macros.rs"), include_str!("fixtures/macros.rs"))
        .map_err(|e| e.to_string())?;
    fs::write(
        output.join("macro-app.rs"),
        include_str!("fixtures/macro-app.rs"),
    )
    .map_err(|e| e.to_string())?;
    let library = output.join("libscarlet_probe_macros.so");
    let mut command = native_compiler(options);
    command
        .args([
            "--crate-type=proc-macro",
            "--crate-name=scarlet_probe_macros",
            "macros.rs",
            "-o",
        ])
        .arg(&library);
    phase(command, output, "proc-macro-build", options.timeout, 0)?;
    check_elf(&library, &options.target)?;
    let other_library = output.join("libscarlet_probe_macros_other.so");
    let mut command = native_compiler(options);
    command
        .args([
            "--crate-type=proc-macro",
            "--crate-name=scarlet_probe_macros_other",
            "macros.rs",
            "-o",
        ])
        .arg(&other_library);
    phase(
        command,
        output,
        "proc-macro-build-other",
        options.timeout,
        0,
    )?;
    check_elf(&other_library, &options.target)?;
    let executable = output.join("macro-app");
    let mut command = native_compiler(options);
    command
        .arg("macro-app.rs")
        .arg("--crate-name=macro_app")
        .arg("--extern")
        .arg(format!("scarlet_probe_macros={}", library.display()))
        .arg("--extern")
        .arg(format!(
            "scarlet_probe_macros_other={}",
            other_library.display()
        ))
        .arg("-o")
        .arg(&executable);
    phase(command, output, "proc-macro-expand", options.timeout, 0)?;
    check_elf(&executable, &options.target)?;
    let stdout = phase(
        Command::new(&executable),
        output,
        "proc-macro-execute",
        options.run_timeout,
        0,
    )?;
    if stdout != MACRO_HELLO.as_bytes() {
        return Err(format!(
            "proc macro program stdout mismatch: {:?}",
            String::from_utf8_lossy(&stdout)
        ));
    }
    fs::write(
        output.join("PROC_MACRO_PASS"),
        "function_like\nattribute\nderive\ntwo_macro_libraries\nthread_tls_destructor\ncompile\nexecute\n",
    )
    .map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC PROC_MACRO PASS");
    Ok(())
}

fn check_elf(path: &Path, target: &str) -> Result<(), String> {
    use std::io::Read;
    let mut header = [0u8; 64];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|e| format!("generated executable: {e}"))?;
    let machine = if target.starts_with("aarch64") {
        183
    } else {
        243
    };
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || header[7] != 83
        || !matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        || u16::from_le_bytes([header[18], header[19]]) != machine
    {
        return Err(
            "compiler output is not a native Scarlet ELF executable for this architecture".into(),
        );
    }
    Ok(())
}

fn cargo_rustflags(options: &Options) -> String {
    let mut flags = format!(
        "-Cpanic=abort -Clinker={}",
        options.linker.as_ref().unwrap().display()
    );
    if let Some(flavor) = &options.linker_flavor {
        flags.push_str(&format!(" -Clinker-flavor={flavor}"));
    }
    if let Some(backend) = &options.backend {
        flags.push_str(&format!(" -Zcodegen-backend={backend}"));
    }
    flags
}

fn check_cargo(options: &Options) -> Result<(), String> {
    let output = &options.output;
    let cargo = options.cargo.as_ref().unwrap();
    let mut version_command = Command::new(cargo);
    version_command.arg("-vV");
    let version = phase(
        version_command,
        output,
        "cargo-version",
        options.run_timeout,
        0,
    )?;
    if !version.starts_with(b"cargo ") {
        return Err("cargo version output is missing".into());
    }

    let fixture = output.join("cargo-fixture");
    fs::create_dir(&fixture).map_err(|e| format!("create Cargo fixture: {e}"))?;
    fs::create_dir(fixture.join("src")).map_err(|e| format!("create Cargo source: {e}"))?;
    fs::write(
        fixture.join("Cargo.toml"),
        "[package]\nname = \"native-cargo-hello\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        fixture.join("src/main.rs"),
        "fn main() { println!(\"SCARLET_NATIVE_CARGO_HELLO_OK\"); }\n",
    )
    .map_err(|e| e.to_string())?;

    let mut command = Command::new(cargo);
    command
        .args([
            "build",
            "--offline",
            "--release",
            "--jobs",
            "1",
            "--target",
            &options.target,
        ])
        .arg("--manifest-path")
        .arg(fixture.join("Cargo.toml"))
        .env("CARGO_HOME", output.join("cargo-home"))
        .env("CARGO_TARGET_DIR", output.join("cargo-target"))
        .env("RUSTC", &options.rustc)
        .env("RUSTFLAGS", cargo_rustflags(options));
    phase(command, output, "cargo-build", options.timeout, 0)?;

    let executable = output
        .join("cargo-target")
        .join(&options.target)
        .join("release/native-cargo-hello");
    check_elf(&executable, &options.target)?;
    let stdout = phase(
        Command::new(&executable),
        output,
        "cargo-execute",
        options.run_timeout,
        0,
    )?;
    if stdout != CARGO_HELLO.as_bytes() {
        return Err(format!(
            "Cargo-built program stdout mismatch: {:?}",
            String::from_utf8_lossy(&stdout)
        ));
    }
    fs::write(output.join("CARGO_PASS"), "offline_build\nexecute\n").map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC CARGO PASS");
    Ok(())
}

fn check_cargo_online(options: &Options) -> Result<(), String> {
    let output = &options.output;
    // ext2 currently rejects rmdir even for an empty temporary directory.
    // rustc removes its .temp-archive directory when finishing every rlib, so
    // build in the guest tmpfs and persist the executable as evidence below.
    let target_dir = Path::new("/tmp/native-cargo-online-target");
    let resolverd = options.resolverd.as_ref().unwrap();
    let mut daemon = Command::new(resolverd)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("start resolverd: {e}"))?;
    let socket = Path::new("/tmp/resolverd.sock");
    for _ in 0..100 {
        if fs::metadata(socket).is_ok() {
            break;
        }
        if let Some(status) = daemon.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("resolverd exited before binding: {status}"));
        }
        thread::sleep(Duration::from_millis(50));
    }
    if fs::metadata(socket).is_err() {
        return Err("resolverd did not bind /tmp/resolverd.sock".into());
    }
    let mut dns = Command::new(env::current_exe().map_err(|e| e.to_string())?);
    dns.arg("--network-dns-child");
    phase(dns, output, "network-dns", Duration::from_secs(15), 0)?;
    let mut tcp = Command::new(env::current_exe().map_err(|e| e.to_string())?);
    tcp.arg("--network-tcp-child");
    phase(tcp, output, "network-tcp", Duration::from_secs(15), 0)?;

    let fixture = output.join("cargo-online-fixture");
    fs::create_dir(&fixture).map_err(|e| e.to_string())?;
    fs::create_dir(fixture.join("src")).map_err(|e| e.to_string())?;
    fs::write(
        fixture.join("Cargo.toml"),
        "[package]\nname = \"native-cargo-online\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\nitoa = \"=1.0.15\"\n",
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        fixture.join("src/main.rs"),
        "fn main() { let mut buffer = itoa::Buffer::new(); println!(\"SCARLET_NATIVE_CARGO_ONLINE_OK={}\", buffer.format(42)); }\n",
    )
    .map_err(|e| e.to_string())?;
    let mut command = Command::new(options.cargo.as_ref().unwrap());
    command
        .args([
            "build",
            "--release",
            "--jobs",
            "1",
            "--target",
            &options.target,
        ])
        .arg("--manifest-path")
        .arg(fixture.join("Cargo.toml"))
        .env("CARGO_HOME", output.join("cargo-online-home"))
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_NET_RETRY", "0")
        .env("CARGO_HTTP_TIMEOUT", "20")
        .env("CARGO_HTTP_DEBUG", "true")
        .env("CARGO_LOG", "network=trace")
        .env("CARGO_HTTP_MULTIPLEXING", "false")
        .env("CARGO_REGISTRIES_CRATES_IO_PROTOCOL", "sparse")
        .env("RUSTC", &options.rustc)
        .env("RUSTFLAGS", cargo_rustflags(options));
    phase(command, output, "cargo-online-build", options.timeout, 0)?;
    let executable = target_dir
        .join(&options.target)
        .join("release/native-cargo-online");
    check_elf(&executable, &options.target)?;
    fs::copy(&executable, output.join("cargo-online-binary"))
        .map_err(|error| format!("persist online-built executable: {error}"))?;
    let stdout = phase(
        Command::new(&executable),
        output,
        "cargo-online-execute",
        options.run_timeout,
        0,
    )?;
    if stdout != CARGO_ONLINE_HELLO.as_bytes() {
        return Err("Cargo online fixture stdout mismatch".into());
    }
    fs::write(
        output.join("CARGO_ONLINE_PASS"),
        "crates.io sparse HTTPS\nitoa 1.0.15\n",
    )
    .map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC CARGO_ONLINE PASS");
    Ok(())
}

fn existing_absolute(path: &Path, label: &str, directory: bool) -> Result<PathBuf, String> {
    // Scarlet's current std Path::is_absolute/canonicalize implementation is
    // not reliable for slash-rooted target paths. has_root is correct, and the
    // probe only receives paths from its rootfs-owned configuration file.
    if !path.has_root() {
        return Err(format!(
            "{label}: expected an absolute path: {}",
            path.display()
        ));
    }
    let metadata = fs::metadata(path).map_err(|error| format!("{label}: {error:?}"))?;
    if directory != metadata.is_dir() {
        let expected = if directory { "directory" } else { "file" };
        return Err(format!(
            "{label}: expected a {expected}: {}",
            path.display()
        ));
    }
    Ok(path.to_owned())
}

fn check_thread_runtime() -> Result<(), String> {
    println!("NATIVE_RUSTC THREAD START");
    let child = thread::Builder::new()
        .name("native-rustc-preflight".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            println!("NATIVE_RUSTC THREAD CHILD");
            THREAD_PREFLIGHT.with(|value| {
                value.set(42);
                value.get()
            })
        })
        .map_err(|error| format!("thread spawn: {error:?}"))?;
    let value = child
        .join()
        .map_err(|_| "thread join: child panicked".to_string())?;
    if value != 42 {
        return Err(format!("thread join: expected 42, got {value}"));
    }
    println!("NATIVE_RUSTC THREAD PASS");

    println!("NATIVE_RUSTC SCOPED_THREAD START");
    let captured = 41;
    let scoped_value = thread::scope(|scope| -> Result<u32, String> {
        let child = thread::Builder::new()
            .name("native-rustc-scoped-preflight".into())
            .stack_size(8 * 1024 * 1024)
            .spawn_scoped(scope, || {
                THREAD_PREFLIGHT.with(|value| {
                    value.set(captured + 1);
                    value.get()
                })
            })
            .map_err(|error| format!("scoped thread spawn: {error:?}"))?;
        child
            .join()
            .map_err(|_| "scoped thread join: child panicked".to_string())
    })?;
    if scoped_value != 42 {
        return Err(format!(
            "scoped thread join: expected 42, got {scoped_value}"
        ));
    }
    println!("NATIVE_RUSTC SCOPED_THREAD PASS");
    Ok(())
}

fn check_process_runtime() -> Result<(), String> {
    println!("NATIVE_RUSTC PROCESS_RUNTIME START");
    let cwd = env::current_dir().map_err(|error| format!("current_dir: {error:?}"))?;
    println!("NATIVE_RUSTC CWD {}", cwd.display());
    let system_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system time before Unix epoch: {error:?}"))?;
    println!(
        "NATIVE_RUSTC SYSTEM_TIME PASS unix_seconds={}",
        system_time.as_secs()
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        args = fs::read_to_string(CONFIG)
            .map_err(|e| format!("read {CONFIG}: {e}; {USAGE}"))?
            .lines()
            .map(str::to_owned)
            .collect();
    }
    let mut options = options(&args)?;
    if !cfg!(target_os = "scarlet") {
        return Err("this probe must run as a native Scarlet program".into());
    }
    if (cfg!(target_arch = "aarch64") && !options.target.starts_with("aarch64"))
        || (cfg!(target_arch = "riscv64") && !options.target.starts_with("riscv64"))
    {
        return Err("probe architecture and requested native target differ".into());
    }
    options.rustc = existing_absolute(&options.rustc, "rustc", false)?;
    options.sysroot = existing_absolute(&options.sysroot, "sysroot", true)?;
    if let Some(startup) = &mut options.c_startup {
        *startup = existing_absolute(startup, "C startup probe", false)?;
        check_elf(startup, &options.target).map_err(|e| format!("C startup probe: {e}"))?;
    }
    if let Some(zlib) = &mut options.zlib {
        *zlib = existing_absolute(zlib, "zlib consumer", false)?;
        check_elf(zlib, &options.target).map_err(|e| format!("zlib consumer: {e}"))?;
    }
    if let Some(sqlite) = &mut options.sqlite {
        *sqlite = existing_absolute(sqlite, "SQLite consumer", false)?;
        check_elf(sqlite, &options.target).map_err(|e| format!("SQLite consumer: {e}"))?;
    }
    if let Some(cargo) = &mut options.cargo {
        *cargo = existing_absolute(cargo, "cargo", false)?;
        check_elf(cargo, &options.target).map_err(|e| format!("Cargo executable: {e}"))?;
    }
    if let Some(resolverd) = &mut options.resolverd {
        *resolverd = existing_absolute(resolverd, "resolverd", false)?;
        check_elf(resolverd, &options.target).map_err(|e| format!("resolverd executable: {e}"))?;
    }
    if let Some(linker) = &mut options.linker {
        *linker = existing_absolute(linker, "linker", false)?;
        check_elf(linker, &options.target).map_err(|e| format!("native linker: {e}"))?;
    }
    // Refuse to reuse old evidence or overwrite any prior run's generated binary.
    if !options.output.has_root() {
        return Err(format!(
            "output: expected an absolute path: {}",
            options.output.display()
        ));
    }
    fs::create_dir(&options.output).map_err(|e| format!("create fresh output directory: {e}"))?;
    let output = &options.output;
    let mode = if options.full {
        "full"
    } else if options.dummy {
        "frontend-dummy"
    } else {
        "frontend"
    };
    fs::write(output.join("mode.txt"), format!("{mode}\n")).map_err(|e| e.to_string())?;
    // Only the native process creates this source and the compiler output.
    fs::write(
        output.join("hello.rs"),
        format!(
            "fn main() {{ println!(\"{}\"); std::process::exit({HELLO_EXIT}); }}\n",
            HELLO.trim_end()
        ),
    )
    .map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC MODE {mode} output={}", output.display());
    check_process_runtime()?;
    check_thread_runtime()?;
    if options.native_fs {
        #[cfg(all(feature = "native-fs", target_os = "scarlet"))]
        {
            println!("NATIVE_RUSTC FILESYSTEM START");
            native_fs::run(output)?;
            println!("NATIVE_RUSTC FILESYSTEM PASS");
        }
        #[cfg(not(all(feature = "native-fs", target_os = "scarlet")))]
        return Err("--native-fs requires building the probe with --features native-fs".into());
    }
    if let Some(startup) = &options.c_startup {
        phase(
            Command::new(startup),
            output,
            "c-startup",
            options.run_timeout,
            43,
        )?;
        fs::write(
            output.join("C_STARTUP_PASS"),
            "C constructor and main exited 43\n",
        )
        .map_err(|e| e.to_string())?;
        println!("NATIVE_RUSTC C_STARTUP PASS");
    }
    if let Some(zlib) = &options.zlib {
        let mut command = Command::new(zlib);
        command.arg(output);
        let stdout = phase(command, output, "zlib", options.run_timeout, 47)?;
        if !String::from_utf8_lossy(&stdout)
            .lines()
            .any(|line| line == ZLIB_HELLO)
        {
            return Err("zlib consumer did not produce its required success marker".into());
        }
        fs::write(
            output.join("ZLIB_PASS"),
            "upstream zlib fixture exited 47\n",
        )
        .map_err(|e| e.to_string())?;
        println!("NATIVE_RUSTC ZLIB PASS");
    }
    if let Some(sqlite) = &options.sqlite {
        for (storage, directory) in [
            ("ext2", output.join("sqlite")),
            ("tmpfs", PathBuf::from("/tmp/native-rustc-sqlite")),
        ] {
            fs::create_dir(&directory)
                .map_err(|e| format!("create fresh SQLite {storage} directory: {e}"))?;
            // Each phase runs in a new process. Recovery must roll back the
            // abandoned transaction before verifying the original committed data.
            for (stage, mode, expected_exit, marker) in [
                ("create", "create", 53, SQLITE_HELLO),
                ("verify", "verify", 53, SQLITE_HELLO),
                ("crash", "crash", 134, SQLITE_CRASH_READY),
                ("recover", "verify", 53, SQLITE_HELLO),
            ] {
                let name = format!("sqlite-{storage}-{stage}");
                let mut command = Command::new(sqlite);
                command.arg(&directory).arg(mode);
                let stdout = phase(command, output, &name, options.run_timeout, expected_exit)?;
                if !String::from_utf8_lossy(&stdout)
                    .lines()
                    .any(|line| line == marker)
                {
                    return Err(format!(
                        "{name}: SQLite consumer did not produce its required success marker"
                    ));
                }
                if stage == "crash" {
                    let journal = fs::read(directory.join("sqlite-roundtrip.db-journal"))
                        .map_err(|e| format!("{name}: read hot journal: {e}"))?;
                    if journal.len() <= 512 || !journal.starts_with(SQLITE_JOURNAL_MAGIC) {
                        return Err(format!("{name}: missing valid hot rollback journal"));
                    }
                    // Preserve evidence before the next process rolls it back.
                    fs::write(output.join(format!("{name}.journal")), journal)
                        .map_err(|e| format!("{name}: preserve hot journal: {e}"))?;
                }
                if stage == "create"
                    && !String::from_utf8_lossy(&stdout)
                        .lines()
                        .any(|line| line == SQLITE_PTHREAD_HELLO)
                {
                    return Err(format!("{name}: missing SQLite pthread acceptance marker"));
                }
            }
        }
        fs::write(
            output.join("SQLITE_PASS"),
            "SQLite create/verify/crash/recover passed on ext2 and tmpfs; crash exit 134, other exits 53\n",
        )
        .map_err(|e| e.to_string())?;
        println!("NATIVE_RUSTC SQLITE PASS");
    }
    // Load the selected backend once up front so the version probe also
    // verifies its DSO dependencies.
    let mut command = compiler(&options);
    command.arg("-Vv");
    let version = phase(command, output, "version", options.timeout, 0)?;
    if !String::from_utf8_lossy(&version)
        .lines()
        .any(|l| l == format!("host: {}", options.target))
    {
        return Err("rustc -Vv did not report the requested Scarlet host".into());
    }
    let mut command = compiler(&options);
    command.args(["--target", &options.target, "--print", "cfg"]);
    let cfg = phase(command, output, "cfg", options.timeout, 0)?;
    if !String::from_utf8_lossy(&cfg)
        .lines()
        .any(|line| line == "target_os=\"scarlet\"")
    {
        return Err("--print cfg did not report target_os=scarlet".into());
    }
    let mut command = compiler(&options);
    command.args([
        "--target",
        &options.target,
        "--edition=2021",
        "-Zno-codegen",
        "hello.rs",
    ]);
    // The dummy backend intentionally rejects linking executables, even with
    // `-Zno-codegen`. An rlib still exercises parsing, analysis, metadata, and
    // the runtime paths this diagnostic is meant to cover.
    if options.dummy {
        command.arg("--crate-type=rlib");
    }
    phase(command, output, "frontend", options.timeout, 0)?;
    if !options.full {
        fs::write(
            output.join("FRONTEND_PASS"),
            format!("mode={mode}\nversion\ncfg\nfrontend\ncodegen_and_execution=not_tested\n"),
        )
        .map_err(|e| e.to_string())?;
        println!("NATIVE_RUSTC FRONTEND PASS");
        return Ok(());
    }
    let executable = output.join("hello");
    let mut command = compiler(&options);
    command
        .args([
            "--target",
            &options.target,
            "--edition=2021",
            "-Cpanic=abort",
            "-Copt-level=0",
        ])
        .arg(format!(
            "-Clinker={}",
            options.linker.as_ref().unwrap().display()
        ));
    if let Some(flavor) = &options.linker_flavor {
        command.arg(format!("-Clinker-flavor={flavor}"));
    }
    command.arg("hello.rs").arg("-o").arg(&executable);
    phase(command, output, "compile", options.timeout, 0)?;
    check_elf(&executable, &options.target)?;
    println!(
        "NATIVE_RUSTC GENERATED bytes={}",
        fs::metadata(&executable).map_err(|e| e.to_string())?.len()
    );
    let stdout = phase(
        Command::new(&executable),
        output,
        "execute",
        options.run_timeout,
        HELLO_EXIT,
    )?;
    if stdout != HELLO.as_bytes() {
        return Err(format!(
            "generated program stdout mismatch: expected {HELLO:?}, got {:?}",
            String::from_utf8_lossy(&stdout)
        ));
    }
    if options.proc_macro {
        check_proc_macro(&options)?;
    }
    if options.cargo.is_some() {
        check_cargo(&options)?;
        if options.cargo_online {
            check_cargo_online(&options)?;
        }
    }
    fs::write(output.join("PASS"), format!("mode=full\nversion\ncfg\nfrontend\ncompile\nexecute\nhello_exit={HELLO_EXIT}\nhello_stdout={HELLO:?}\n"))
        .map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC FULL PASS");
    Ok(())
}

fn main() {
    if env::args().nth(1).as_deref() == Some("--network-tcp-child") {
        use std::net::{TcpStream, ToSocketAddrs};
        let result = ("index.crates.io", 443)
            .to_socket_addrs()
            .and_then(|mut addrs| {
                let address = addrs.next().ok_or(std::io::ErrorKind::NotFound)?;
                TcpStream::connect_timeout(&address, Duration::from_secs(5))
            });
        match result {
            Ok(_) => {
                println!("NETWORK_TCP_OK index.crates.io:443");
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("NETWORK_TCP_FAIL {error:?}");
                std::process::exit(1);
            }
        }
    }
    if env::args().nth(1).as_deref() == Some("--network-dns-child") {
        use std::net::ToSocketAddrs;
        match ("index.crates.io", 443).to_socket_addrs() {
            Ok(addrs) => {
                let addresses: Vec<_> = addrs.collect();
                println!("NETWORK_DNS {addresses:?}");
                std::process::exit(i32::from(addresses.is_empty()));
            }
            Err(error) => {
                eprintln!("NETWORK_DNS_FAIL {error:?}");
                std::process::exit(1);
            }
        }
    }
    #[cfg(all(feature = "native-fs", target_os = "scarlet"))]
    if let Some(argument) = env::args().nth(1) {
        if matches!(
            argument.as_str(),
            "--libc-assert-child" | "--libc-abort-child"
        ) {
            native_fs::runtime_child(&argument);
        }
    }
    #[cfg(all(feature = "native-fs", target_os = "scarlet"))]
    allocation_failure::check();
    if let Err(error) = run() {
        eprintln!("NATIVE_RUSTC FAIL {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &[&str]) -> Result<Options, String> {
        options(
            &["/rustc", "/sysroot", "aarch64-unknown-scarlet", "/tmp/new"]
                .into_iter()
                .chain(extra.iter().copied())
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn legacy_modes_never_select_full() {
        assert!(!parse(&[]).unwrap().full);
        assert!(parse(&["--dummy"]).unwrap().dummy);
        assert!(!parse(&["--dummy"]).unwrap().full);
    }

    #[test]
    fn full_requires_a_real_linker_and_forbids_dummy() {
        assert!(parse(&["--full"]).is_err());
        assert!(parse(&["--full", "--linker", "/lld", "--dummy"]).is_err());
        let full = parse(&[
            "--full",
            "--linker",
            "/lld",
            "--backend",
            "/cg.so",
            "--linker-flavor",
            "gnu-lld",
        ])
        .unwrap();
        assert!(full.full);
        assert_eq!(full.backend.as_deref(), Some("/cg.so"));
        assert!(parse(&["--proc-macro"]).is_err());
        assert!(
            parse(&["--full", "--linker", "/lld", "--proc-macro"])
                .unwrap()
                .proc_macro
        );
    }

    #[test]
    fn sqlite_consumer_requires_a_nonempty_path() {
        assert!(parse(&["--sqlite"]).is_err());
        assert!(parse(&["--sqlite", ""]).is_err());
        assert_eq!(
            parse(&["--sqlite", "/system/bin/native-sqlite-probe"])
                .unwrap()
                .sqlite
                .as_deref(),
            Some(Path::new("/system/bin/native-sqlite-probe"))
        );
    }

    #[test]
    fn cargo_requires_full_mode_and_a_path() {
        assert!(parse(&["--cargo", "/cargo"]).is_err());
        assert!(parse(&["--full", "--linker", "/lld", "--cargo"]).is_err());
        assert_eq!(
            parse(&["--full", "--linker", "/lld", "--cargo", "/cargo"])
                .unwrap()
                .cargo
                .as_deref(),
            Some(Path::new("/cargo"))
        );
        assert!(parse(&["--full", "--linker", "/lld", "--cargo-online"]).is_err());
        assert!(parse(&["--full", "--linker", "/lld", "--resolverd", "/resolverd"]).is_err());
        assert!(
            parse(&[
                "--full",
                "--linker",
                "/lld",
                "--cargo",
                "/cargo",
                "--cargo-online"
            ])
            .is_err()
        );
        let online = parse(&[
            "--full",
            "--linker",
            "/lld",
            "--cargo",
            "/cargo",
            "--cargo-online",
            "--resolverd",
            "/resolverd",
        ])
        .unwrap();
        assert!(online.cargo_online);
        assert_eq!(online.resolverd.as_deref(), Some(Path::new("/resolverd")));
    }

    #[test]
    fn bounded_timeouts_and_unknown_flags() {
        for value in ["0", "86401", "-1", "nan"] {
            assert!(parse(&["--timeout", value]).is_err());
        }
        assert_eq!(
            parse(&["--run-timeout", "3"])
                .unwrap()
                .run_timeout
                .as_secs(),
            3
        );
        assert!(parse(&["--full", "--unknown"]).is_err());
    }
}
