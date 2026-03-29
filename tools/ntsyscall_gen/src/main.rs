mod codegen;
mod pe;
mod scanner;

use std::env;
use std::path::{Path, PathBuf};

use codegen::{generate_reference_table, generate_syscall_table_rs};
use pe::PeFile;
use scanner::{parse_text_table, scan_nt_syscalls};

const VALID_ARCHS: &[&str] = &["aarch64", "x86_64"];

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args();
    let prog = args.next().unwrap_or_else(|| "ntsyscall_gen".to_string());

    let mut text_mode = false;
    let mut arch = None;
    let mut input_path = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--text" => text_mode = true,
            "--arch" => {
                let a = args.next().ok_or_else(|| {
                    format!(
                        "--arch requires a value (valid: {})\n{}",
                        VALID_ARCHS.join(", "),
                        usage(&prog)
                    )
                })?;
                if !VALID_ARCHS.contains(&a.as_str()) {
                    return Err(format!(
                        "unknown arch '{a}' (valid: {})\n{}",
                        VALID_ARCHS.join(", "),
                        usage(&prog)
                    ));
                }
                arch = Some(a);
            }
            _ if input_path.is_none() => input_path = Some(arg),
            _ => return Err(format!("unexpected argument: {arg}\n{}", usage(&prog))),
        }
    }

    let input_path = input_path.ok_or_else(|| usage(&prog))?;
    let input_path = PathBuf::from(&input_path);

    let arch = arch.ok_or_else(|| usage(&prog))?;

    let project_root = find_project_root()?;
    let output_path = project_root.join(format!("kernel/src/abi/windows/{arch}/syscall_table.rs"));
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed creating '{}': {e}", parent.display()))?;
    }

    let (entries, version, source_hint) = if text_mode {
        let entries = parse_text_table(&input_path)?;
        if entries.is_empty() {
            return Err("no syscall entries found in text file".to_string());
        }
        let version = extract_version_from_path(&input_path);
        let hint = format!(
            "cargo run --release --manifest-path tools/ntsyscall_gen/Cargo.toml -- --arch {arch} --text {}",
            input_path.display()
        );
        (entries, version, hint)
    } else {
        let bytes = std::fs::read(&input_path)
            .map_err(|e| format!("failed to read '{}': {e}", input_path.display()))?;
        let pe = PeFile::parse(&bytes).map_err(|e| format!("{e}"))?;
        let entries = scan_nt_syscalls(&pe)?;
        if entries.is_empty() {
            return Err("no Nt*/Zw* syscall stubs found in export table".to_string());
        }
        let hint = format!(
            "cargo run --release --manifest-path tools/ntsyscall_gen/Cargo.toml -- --arch {arch} {}",
            input_path.display()
        );
        (entries, pe.version_string().map(str::to_string), hint)
    };

    let generated = generate_syscall_table_rs(
        &entries,
        version.clone(),
        source_hint,
        input_path.display().to_string(),
    );

    std::fs::write(&output_path, generated)
        .map_err(|e| format!("failed writing '{}': {e}", output_path.display()))?;

    let reference_path = output_path.with_extension("reference.md");
    let reference = generate_reference_table(&entries, version, input_path.display().to_string());
    std::fs::write(&reference_path, reference)
        .map_err(|e| format!("failed writing '{}': {e}", reference_path.display()))?;

    println!("arch: {arch}");
    println!("generated {} entries", entries.len());
    println!("wrote: {}", output_path.display());
    println!("wrote: {}", reference_path.display());

    Ok(())
}

fn extract_version_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if let Some(idx) = stem.find("26") {
        let version = &stem[idx..];
        if version.len() >= 5 && version.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!(
                "Windows 11 build {version} (from hfiref0x/SyscallTables)"
            ));
        }
    }
    None
}

fn usage(program: &str) -> String {
    format!(
        "usage:\n  {program} --arch <aarch64|x86_64> [--text] <input-file>\n\nmodes:\n  (default)  parse ntdll.dll PE binary\n  --text     parse hfiref0x/SyscallTables text file (Name\\tNumber)\n\nexamples:\n  {program} --arch aarch64 /mnt/c/Windows/System32/ntdll.dll\n  {program} --arch x86_64 --text x64_ntos_26100.txt"
    )
}

fn find_project_root() -> Result<PathBuf, String> {
    let cwd = env::current_dir().map_err(|e| format!("failed to read current directory: {e}"))?;
    find_project_root_from(&cwd)
}

fn find_project_root_from(start: &Path) -> Result<PathBuf, String> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("kernel").join("src").join("abi");
        if candidate.is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err("could not locate project root (expected kernel/src/abi)".to_string())
}
