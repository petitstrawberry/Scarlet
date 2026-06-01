use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=ui/slint_demo.slint");
    println!("cargo:rerun-if-changed=build.rs");

    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);

    slint_build::compile_with_config("ui/slint_demo.slint", config)
        .expect("Failed to compile Slint UI");

    build_ring_aarch64_asm();
}

fn build_ring_aarch64_asm() {
    let target = env::var("TARGET").unwrap_or_default();
    if target != "aarch64-unknown-scarlet-elf" {
        return;
    }

    let Some(ring_dir) = find_ring_source_dir() else {
        return;
    };
    println!("cargo:rerun-if-changed={}", ring_dir.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set"));
    let asm_dir = ring_dir.join("pregenerated");
    let include_dir = ring_dir.join("include");
    let asm_sources = [
        "aesv8-armx-linux64.S",
        "aesv8-gcm-armv8-linux64.S",
        "armv8-mont-linux64.S",
        "chacha-armv8-linux64.S",
        "chacha20_poly1305_armv8-linux64.S",
        "ghash-neon-armv8-linux64.S",
        "ghashv8-armx-linux64.S",
        "p256-armv8-asm-linux64.S",
        "sha256-armv8-linux64.S",
        "sha512-armv8-linux64.S",
        "vpaes-armv8-linux64.S",
    ];

    let mut objects = Vec::new();
    for source in asm_sources {
        let source_path = asm_dir.join(source);
        let object_path = out_dir.join(format!("{}.o", source.trim_end_matches(".S")));
        let status = Command::new("clang")
            .arg("--target=aarch64-unknown-none-elf")
            .arg("-DRING_CORE_NOSTDLIBINC")
            .arg("-fno-stack-protector")
            .arg("-Wno-unused-command-line-argument")
            .arg("-I")
            .arg(&include_dir)
            .arg("-I")
            .arg(&asm_dir)
            .arg("-c")
            .arg(&source_path)
            .arg("-o")
            .arg(&object_path)
            .status()
            .expect("failed to run clang for ring AArch64 asm");
        assert!(
            status.success(),
            "failed to compile ring AArch64 asm source {}",
            source_path.display()
        );
        objects.push(object_path);
    }

    for bin in ["yt", "yt-gui"] {
        for object in &objects {
            println!("cargo:rustc-link-arg-bin={}={}", bin, object.display());
        }
    }
}

fn find_ring_source_dir() -> Option<PathBuf> {
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")))?;
    let registry_src = cargo_home.join("registry").join("src");
    let entries = fs::read_dir(registry_src).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join("ring-0.17.14");
        if candidate.join("pregenerated").is_dir() {
            return Some(candidate);
        }
    }
    None
}
