fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("scarlet") {
        let aarch64 = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64");
        let (base, page_size) = if aarch64 {
            // Keep the native interpreter in the hosted AArch64 image window
            // with separate permissions on both 4 KiB and 16 KiB hosts.
            ("--image-base=0x7801800000", "max-page-size=16384")
        } else {
            ("--image-base=0x40000000", "max-page-size=4096")
        };
        // Keep the interpreter away from the kernel's default PIE load bias.
        // Native std supplies its own `_start`; our assembly shim captures SP
        // before that startup has a chance to consume the initial stack.
        for argument in [
            "--entry=_scarlet_ld_start",
            base,
            "--no-dynamic-linker",
            "-static",
            "--no-pie",
            "-z",
            page_size,
        ] {
            println!("cargo:rustc-link-arg={argument}");
        }
        if aarch64 {
            println!("cargo:rustc-link-arg=-z");
            println!("cargo:rustc-link-arg=separate-loadable-segments");
        }
    }
}
