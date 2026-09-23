use std::{env, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    assert!(
        command.status().expect("start C fixture tool").success(),
        "failed: {command:?}"
    );
}

fn main() {
    let fixtures = [
        "native",
        "strings",
        "descriptor",
        "stdio",
        "algorithms",
        "positioned",
        "path",
        "runtime",
        "threading",
        "pthread_sync",
    ];
    for fixture in fixtures {
        println!("cargo:rerun-if-changed=../../user/lib/scarlet-libc/tests/{fixture}.c");
    }
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
        Ok("riscv64") => ("riscv64-unknown-elf", &["-march=rv64gc", "-mabi=lp64d"]),
        arch => panic!("unsupported native C probe target: {arch:?}"),
    };
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../user/lib/scarlet-libc");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let clang = env::var_os("SCARLET_PROBE_CC").unwrap_or_else(|| "clang".into());
    let resource = Command::new(&clang)
        .arg("-print-resource-dir")
        .output()
        .unwrap();
    assert!(
        resource.status.success(),
        "Clang resource directory query failed"
    );
    let builtin = PathBuf::from(String::from_utf8(resource.stdout).unwrap().trim()).join("include");
    assert!(builtin.join("stddef.h").is_file());
    let mut objects = Vec::new();
    for fixture in fixtures {
        let object = output.join(format!("{fixture}.o"));
        let mut command = Command::new(&clang);
        command
            .args(arch_flags)
            .args([
                "-target",
                target,
                "-std=c11",
                "-ffreestanding",
                "-fno-builtin",
                "-fno-stack-protector",
                "-nostdinc",
                "-isystem",
            ])
            .arg(&builtin)
            .args(["-O2", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(source.join("include"))
            .arg("-c")
            .arg(source.join(format!("tests/{fixture}.c")))
            .arg("-o")
            .arg(&object);
        for variable in [
            "CPATH",
            "C_INCLUDE_PATH",
            "CPLUS_INCLUDE_PATH",
            "OBJC_INCLUDE_PATH",
        ] {
            command.env_remove(variable);
        }
        run(&mut command);
        objects.push(object);
    }
    run(
        Command::new(env::var_os("SCARLET_PROBE_AR").unwrap_or_else(|| "llvm-ar".into()))
            .arg("crs")
            .arg(output.join("libscarlet_libc_probe.a"))
            .args(objects),
    );
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=scarlet_libc_probe");
}
