// SPDX-License-Identifier: MIT
fn main() {
    let version = std::process::Command::new("/bin/java")
        .args(["-XX:ErrorFile=/tmp/openjdk-error.log", "-version"])
        .status();
    if !matches!(version, Ok(ref status) if status.success()) {
        println!("OPENJDK_GUEST FAIL version {version:?}");
        report_crash();
    }
    let result = std::process::Command::new("/bin/java")
        .args([
            "-XX:ErrorFile=/tmp/openjdk-error.log",
            "-Xlog:gc",
            "-cp",
            "/app",
            "Hello",
            "Scarlet",
            "いちご🍓",
        ])
        .status();
    match result {
        Ok(status) if status.success() => println!("OPENJDK_GUEST PASS child exit=0"),
        other => {
            println!("OPENJDK_GUEST FAIL {other:?}");
            report_crash();
        }
    }
    #[cfg(jit)]
    {
        let result = std::process::Command::new("/bin/java")
            .args([
                "-XX:ErrorFile=/tmp/openjdk-error.log",
                "-Xcomp",
                "-XX:-BackgroundCompilation",
                "-XX:+PrintCompilation",
                "-XX:CompileCommand=compileonly,Jit::*",
                "-XX:CompileCommand=dontinline,Jit::*",
                "-cp",
                "/app",
                "Jit",
            ])
            .status();
        match result {
            Ok(status) if status.success() => println!("OPENJDK_GUEST JIT child exit=0"),
            other => {
                println!("OPENJDK_GUEST FAIL JIT {other:?}");
                report_crash();
            }
        }
    }
    // This disposable validation VM owns its lifecycle; the Java launcher does not.
    unsafe {
        core::arch::asm!("svc #0", inout("x0") 0usize => _, inout("x1") 0usize => _,
            inout("x8") 1000usize => _, clobber_abi("C"));
    }
    std::process::exit(1);
}

fn report_crash() {
    if let Ok(log) = std::fs::read_to_string("/tmp/openjdk-error.log") {
        println!("{log}");
    }
}
