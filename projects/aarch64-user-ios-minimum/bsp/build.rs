use std::{env, path::PathBuf, process::Command};

const SWIFT_SOURCES: &[&str] = &["src/ScarletApp.swift", "src/TerminalView.swift"];

fn xcrun_path(sdk: &str, args: &[&str]) -> PathBuf {
    let output = Command::new("/usr/bin/xcrun")
        .args(["--sdk", sdk])
        .args(args)
        .output()
        .expect("cannot start Xcode tool lookup");
    assert!(output.status.success(), "Xcode lookup failed for {args:?}");
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn main() {
    let (sdk, target) = match env::var("TARGET").as_deref() {
        Ok("aarch64-apple-ios") => ("iphoneos", "arm64-apple-ios16.0"),
        Ok("aarch64-apple-ios-sim") => ("iphonesimulator", "arm64-apple-ios16.0-simulator"),
        _ => panic!("The iOS BSP requires an arm64 iOS target"),
    };
    let object = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("ScarletApp.o");
    let swiftc = xcrun_path(sdk, &["--find", "swiftc"]);
    let sdkroot = xcrun_path(sdk, &["--show-sdk-path"]);
    let optimization = if env::var("OPT_LEVEL").as_deref() == Ok("0") {
        "-Onone"
    } else {
        "-O"
    };
    assert!(
        Command::new(&swiftc)
            .args(["-target", target, "-sdk"])
            .arg(&sdkroot)
            .args([
                "-swift-version",
                "6",
                "-warnings-as-errors",
                optimization,
                "-whole-module-optimization",
                "-parse-as-library",
                "-import-objc-header",
                "src/ScarletHost.h",
                "-c",
            ])
            .args(SWIFT_SOURCES)
            .arg("-o")
            .arg(&object)
            .status()
            .expect("cannot start Xcode Swift compiler")
            .success(),
        "cannot compile the Swift app"
    );

    for source in SWIFT_SOURCES.iter().copied().chain(["src/ScarletHost.h"]) {
        println!("cargo:rerun-if-changed={source}");
    }
    for variable in ["DEVELOPER_DIR", "TOOLCHAINS"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    // Swift emits Darwin autolink directives. Give Cargo's clang link the
    // same Swift library search paths and runtime rpath as swiftc.
    let swift_libs = swiftc
        .parent()
        .unwrap()
        .join("../lib/swift")
        .join(sdk);
    for libs in [swift_libs, sdkroot.join("usr/lib/swift")] {
        println!("cargo:rustc-link-search=native={}", libs.display());
    }
    println!("cargo:rustc-link-arg-bin=scarlet=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rustc-link-arg-bin=scarlet={}", object.display());
    for framework in ["SwiftUI", "UIKit", "Foundation", "Security", "CoreFoundation", "CoreGraphics"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
