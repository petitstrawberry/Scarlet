use std::{env, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    assert!(
        command.status().expect("start C fixture tool").success(),
        "failed: {command:?}"
    );
}

fn main() {
    println!("cargo:rerun-if-changed=../../user/lib/scarlet-libc/tests/native.c");
    println!("cargo:rerun-if-changed=../../user/lib/scarlet-libc/include");
    println!("cargo:rerun-if-env-changed=SCARLET_PROBE_CC");
    println!("cargo:rerun-if-env-changed=SCARLET_PROBE_AR");
    if env::var_os("CARGO_FEATURE_NATIVE_FS").is_none()
        || env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("scarlet")
    {
        return;
    }
    let (target, arch_flags): (&str, &[&str]) = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => ("aarch64-none-elf", &[]),
        // Match Rust's RV64GC/lp64d target and its executable CRT.
        Ok("riscv64") => ("riscv64-unknown-elf", &["-march=rv64gc", "-mabi=lp64d"]),
        arch => panic!("unsupported native C probe target: {arch:?}"),
    };
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../user/lib/scarlet-libc");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = output.join("native.o");
    run(
        Command::new(env::var_os("SCARLET_PROBE_CC").unwrap_or_else(|| "clang".into()))
            .args(arch_flags)
            .args([
                "-target",
                target,
                "-ffreestanding",
                "-fno-builtin",
                "-fno-stack-protector",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
            ])
            .arg(source.join("include"))
            .arg("-c")
            .arg(source.join("tests/native.c"))
            .arg("-o")
            .arg(&object),
    );
    run(
        Command::new(env::var_os("SCARLET_PROBE_AR").unwrap_or_else(|| "llvm-ar".into()))
            .arg("crs")
            .arg(output.join("libscarlet_libc_probe.a"))
            .arg(object),
    );
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=scarlet_libc_probe");
}
