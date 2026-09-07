use std::path::Path;

fn parse_depends(content: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut rest = content;

    while let Some(depends_pos) = rest.find("depends") {
        rest = &rest[depends_pos + "depends".len()..];
        let Some(eq_pos) = rest.find('=') else {
            break;
        };
        rest = &rest[eq_pos + 1..];
        let Some(open_pos) = rest.find('[') else {
            break;
        };
        rest = &rest[open_pos + 1..];
        let Some(close_pos) = rest.find(']') else {
            break;
        };

        let array = &rest[..close_pos];
        for item in array.split(',') {
            let trimmed = item.trim();
            if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
                deps.push(trimmed[1..trimmed.len() - 1].to_string());
            }
        }
        break;
    }

    deps
}

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

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let module_toml = Path::new(&manifest_dir).join("module.toml");
    println!("cargo:rerun-if-changed={}", module_toml.display());

    if module_toml.exists() {
        let content = std::fs::read_to_string(&module_toml).expect("failed to read module.toml");
        let depends = parse_depends(&content);
        println!("cargo:rustc-env=SCARLET_LSM_DEPENDS={}", depends.join(","));
    } else {
        println!("cargo:rustc-env=SCARLET_LSM_DEPENDS=");
    }
}
