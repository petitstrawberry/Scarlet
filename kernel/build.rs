fn main() {
    let rustc_version = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = std::process::Command::new(rustc_version)
        .arg("--version")
        .output()
        .expect("failed to run rustc --version");
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("cargo:rustc-env=RUSTC_VERSION={version}");

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=TARGET={target}");

    // Ensure generated_symbols.rs exists so include! in symbol.rs never fails.
    // The BSP build.rs (or cargo-scarlet scaffold) will overwrite this with real symbols.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let symbols_path = std::path::Path::new(&manifest_dir).join("src/lsm/generated_symbols.rs");
    if !symbols_path.exists() {
        let empty = r#"#[allow(dead_code)]

#[unsafe(link_section = ".lsm_symbols")]
#[used]
static _FORCE_SECTION: usize = 0;

#[allow(dead_code)]
static KERNEL_SYMBOLS: [(&'static str, usize); 0] = [];

pub fn get_kernel_symbols() -> &'static [(&'static str, usize)] { &KERNEL_SYMBOLS }
"#;
        std::fs::write(&symbols_path, empty)
            .expect("failed to write generated_symbols.rs placeholder");
    }
}
