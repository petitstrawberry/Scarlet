//! Run on Scarlet after staging a native compiler. No shell status parsing required.
use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn phase(rustc: &Path, sysroot: &Path, output: &Path, name: &str, args: &[&str], dummy: bool)
    -> Result<String, String>
{
    let stdout_path = output.join(format!("{name}.stdout"));
    let stderr_path = output.join(format!("{name}.stderr"));
    let stdout = File::create(&stdout_path).map_err(|e| e.to_string())?;
    let stderr = File::create(&stderr_path).map_err(|e| e.to_string())?;
    let mut command = Command::new(rustc);
    command.args(args).arg("--sysroot").arg(sysroot).current_dir(output)
        .stdin(Stdio::null()).stdout(stdout).stderr(stderr);
    if dummy {
        command.arg("-Zcodegen-backend=dummy");
    }
    fs::write(output.join(format!("{name}.command")), format!("{command:?}\n"))
        .map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC START {name}");
    let mut child = command.spawn().map_err(|e| format!("spawn: {e}"))?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => (),
            Err(e) => return Err(format!("wait: {e}")),
        }
        if start.elapsed() > Duration::from_secs(120) {
            let killed = child.kill();
            if killed.is_ok() {
                let _ = child.wait();
            }
            return Err(format!("timed out after 120 seconds; kill result: {killed:?}"));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let out = fs::read_to_string(stdout_path).map_err(|e| e.to_string())?;
    let err = fs::read_to_string(stderr_path).map_err(|e| e.to_string())?;
    print!("{out}");
    eprint!("{err}");
    if !status.success() {
        return Err(format!("{name}: {status}"));
    }
    Ok(out)
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if !(args.len() == 5 || args.len() == 6 && args[5] == "--dummy") {
        return Err("usage: native-rustc-probe RUSTC SYSROOT TARGET NEW_OUTPUT_DIR [--dummy]".into());
    }
    if !cfg!(target_os = "scarlet") {
        return Err("this probe must run as a native Scarlet program".into());
    }
    let rustc = fs::canonicalize(&args[1]).map_err(|e| format!("rustc: {e}"))?;
    let sysroot = fs::canonicalize(&args[2]).map_err(|e| format!("sysroot: {e}"))?;
    let target = &args[3];
    if !matches!(target.as_str(), "riscv64gc-unknown-scarlet" | "aarch64-unknown-scarlet") {
        return Err("expected a supported 64-bit native Scarlet target".into());
    }
    let output = PathBuf::from(&args[4]);
    // Refuse to reuse old evidence or accidentally overwrite a previous run.
    fs::create_dir(&output).map_err(|e| format!("create fresh output directory: {e}"))?;
    let output = fs::canonicalize(output).map_err(|e| e.to_string())?;
    let dummy = args.len() == 6;
    fs::write(output.join("hello.rs"), "fn main() { println!(\"native rustc hello\"); }\n")
        .map_err(|e| e.to_string())?;
    fs::write(output.join("mode.txt"), if dummy { "dummy\n" } else { "default\n" })
        .map_err(|e| e.to_string())?;
    let version = phase(&rustc, &sysroot, &output, "version", &["-Vv"], dummy)?;
    if !version.lines().any(|l| l == format!("host: {target}")) {
        return Err("rustc -Vv did not report the requested Scarlet host".into());
    }
    println!("NATIVE_RUSTC PASS version");
    let cfg = phase(&rustc, &sysroot, &output, "cfg", &["--print", "cfg", "--target", target], dummy)?;
    if !cfg.lines().any(|l| l == "target_os=\"scarlet\"") {
        return Err("--print cfg did not report target_os=scarlet".into());
    }
    println!("NATIVE_RUSTC PASS cfg");
    phase(&rustc, &sysroot, &output, "frontend",
        &["--target", target, "--edition=2021", "-Zno-codegen", "hello.rs"], dummy)?;
    println!("NATIVE_RUSTC PASS frontend");
    fs::write(output.join("PASS"), "version\ncfg\nfrontend\n").map_err(|e| e.to_string())?;
    println!("NATIVE_RUSTC PASS all mode={}", if dummy { "dummy" } else { "default" });
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("NATIVE_RUSTC FAIL {error}");
        std::process::exit(1);
    }
}
