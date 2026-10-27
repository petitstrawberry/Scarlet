use std::{env, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let upstream = root.join("vendor/wpa_supplicant-2.12/src");
    let native = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("scarlet");
    let mut build = cc::Build::new();
    build
        .include(&upstream)
        .include(upstream.join("utils"))
        .include(root.join("c"));
    build
        .flag("-include")
        .flag(root.join("c/os_config.h").to_str().unwrap());
    build
        .warnings(false)
        .opt_level(2)
        .flag("-ffunction-sections")
        .flag("-fdata-sections");
    for flag in [
        "CONFIG_NO_STDOUT_DEBUG",
        "CONFIG_NO_WPA_MSG",
        "CONFIG_NO_RANDOM_POOL",
        "CONFIG_NO_TKIP",
        "CONFIG_NO_RC4",
        "CONFIG_CRYPTO_INTERNAL",
        "CONFIG_INTERNAL_AES",
        "CONFIG_INTERNAL_SHA1",
        "CONFIG_INTERNAL_SHA256",
        "CONFIG_INTERNAL_SHA384",
        "CONFIG_INTERNAL_SHA512",
        "CONFIG_INTERNAL_MD5",
        "CONFIG_SHA256",
        "CONFIG_SHA384",
        "CONFIG_SHA512",
    ] {
        build.define(flag, None);
    }
    if native {
        let runtime = PathBuf::from(
            env::var("SCARLET_NATIVE_RUNTIME")
                .expect("use nix develop: SCARLET_NATIVE_RUNTIME required"),
        );
        let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
        let triple = match arch.as_str() {
            "aarch64" => "aarch64-unknown-none-elf",
            "riscv32" => "riscv32-unknown-none-elf",
            "riscv64" => "riscv64-unknown-none-elf",
            _ => panic!("unsupported native architecture"),
        };
        build.compiler(env::var("SCARLET_NATIVE_CC").unwrap());
        build
            .flag(&format!("--target={triple}"))
            .flag("-ffreestanding")
            .flag("-nostdlibinc")
            .flag("-fno-stack-protector")
            .include(runtime.join("include"))
            .define("__scarlet__", None);
        if arch == "riscv32" {
            build.flag("-march=rv32gc").flag("-mabi=ilp32d");
        }
    }
    for file in include_str!("sources.txt")
        .lines()
        .filter(|l| !l.is_empty())
    {
        build.file(upstream.join(file));
    }
    build.file(root.join("c/format.c"));
    build
        .file(root.join("c/native.c"))
        .compile("scarlet_wpa_engine");
    // C allocation uses the owning Rust runtime; do not link the SDK static
    // Rust runtime a second time through libscarlet_c.
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=sources.txt");
}
