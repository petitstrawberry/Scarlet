use std::path::Path;
use std::process::Command;

const KERNEL_SYMBOLS_RELATIVE: &str = "../../kernel/src/lsm/generated_symbols.rs";

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = std::env::var("TARGET").unwrap();
    let profile = std::env::var("PROFILE").unwrap();
    let target_dir =
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| format!("{manifest_dir}/target"));

    let binary_path = format!("{target_dir}/{target}/{profile}/scarlet");
    let kernel_symbols_path = format!("{manifest_dir}/{KERNEL_SYMBOLS_RELATIVE}");

    if Path::new(&binary_path).exists() {
        extract_symbols(&binary_path, &kernel_symbols_path);
    } else {
        generate_empty_symbols(&kernel_symbols_path);
    }
}

fn extract_symbols(binary_path: &str, output_path: &str) {
    let output = Command::new("nm")
        .args([
            "--defined-only",
            "--extern-only",
            "-g",
            "--no-sort",
            binary_path,
        ])
        .output()
        .expect("failed to run nm");

    if !output.status.success() {
        eprintln!("cargo-scarlet [build.rs]: nm failed, generating empty symbols");
        generate_empty_symbols(output_path);
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut symbols: Vec<(String, String)> = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let addr = parts[0];
        let name = parts[2];

        if name.is_empty() {
            continue;
        }

        let skip = match name {
            "_GLOBAL_OFFSET_TABLE_" | "_DYNAMIC" => true,
            _ if name.starts_with("__") && name.ends_with("_START") => true,
            _ if name.starts_with("__") && name.ends_with("_END") => true,
            _ => false,
        };

        if !skip {
            symbols.push((name.to_string(), addr.to_string()));
        }
    }

    let count = symbols.len();
    let mut content = String::new();
    content.push_str("#[allow(dead_code)]\n\n");
    content.push_str("#[unsafe(link_section = \".lsm_symbols\")]\n");
    content.push_str("#[used]\n");
    content.push_str("static _FORCE_SECTION: usize = 0;\n\n");
    content.push_str("#[allow(dead_code)]\n");
    content.push_str(&format!(
        "static KERNEL_SYMBOLS: [(&'static str, usize); {count}] = [\n"
    ));
    for (name, addr) in &symbols {
        content.push_str(&format!("    (\"{name}\", 0x{addr}),\n"));
    }
    content.push_str("];\n\n");
    content.push_str(
        "pub fn get_kernel_symbols() -> &'static [(&'static str, usize)] { &KERNEL_SYMBOLS }\n",
    );

    write_if_changed(output_path, &content);
    eprintln!(
        "cargo-scarlet [build.rs]: extracted {count} kernel symbols from {}",
        binary_path
    );
}

fn generate_empty_symbols(output_path: &str) {
    let content = r#"#[allow(dead_code)]

#[unsafe(link_section = ".lsm_symbols")]
#[used]
static _FORCE_SECTION: usize = 0;

#[allow(dead_code)]
static KERNEL_SYMBOLS: [(&'static str, usize); 0] = [];

pub fn get_kernel_symbols() -> &'static [(&'static str, usize)] { &KERNEL_SYMBOLS }
"#;
    write_if_changed(output_path, content);
}

fn write_if_changed(path: &str, contents: &str) {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == contents {
            return;
        }
    }
    std::fs::write(path, contents).expect("failed to write generated_symbols.rs");
}
