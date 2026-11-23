use std::{env, path::PathBuf, process::Command};

fn main() {
    let sdk = match env::var("TARGET").as_deref() {
        Ok("aarch64-apple-ios") => "iphoneos",
        Ok("aarch64-apple-ios-sim") => "iphonesimulator",
        _ => panic!("The iOS BSP requires an arm64 iOS target"),
    };
    let target = match sdk {
        "iphoneos" => "arm64-apple-ios16.0",
        _ => "arm64-apple-ios16.0-simulator",
    };
    let path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("app.o");
    let mut clang = Command::new("/usr/bin/xcrun");
    clang
        .args([
            "--sdk", sdk, "clang", "-target", target, "-fobjc-arc",
            "-Wall", "-Wextra", "-Werror", "-c", "src/main.m", "-o",
        ])
        .arg(&path);
    assert!(clang.status().expect("cannot start Xcode clang").success());
    println!("cargo:rerun-if-changed=src/main.m");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rustc-link-arg-bin=scarlet={}", path.display());
    for framework in ["UIKit", "Foundation", "Security", "CoreFoundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
