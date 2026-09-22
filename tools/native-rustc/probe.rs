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
const MACRO_HELLO: &str = "SCARLET_NATIVE_PROC_MACRO_OK=42\n";
const CONFIG: &str = "/etc/native-rustc-probe.args";
const USAGE: &str = "usage: native-rustc-probe RUSTC SYSROOT TARGET NEW_OUTPUT_DIR [--dummy] [--full --linker PATH] [--proc-macro] [--backend PATH_OR_NAME] [--linker-flavor FLAVOR] [--timeout SECONDS] [--run-timeout SECONDS]; no arguments reads /etc/native-rustc-probe.args (one argument per line)";

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
            "--backend" | "--linker" | "--linker-flavor" | "--timeout" | "--run-timeout" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                if value.is_empty() {
                    return Err(format!("empty value for {flag}"));
                }
                match flag.as_str() {
                    "--backend" => result.backend = Some(value.clone()),
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
    fs::write(output.join("PASS"), format!("mode=full\nversion\ncfg\nfrontend\ncompile\nexecute\nhello_exit={HELLO_EXIT}\nhello_stdout={HELLO:?}\n"))
        .map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC FULL PASS");
    Ok(())
}

fn main() {
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
