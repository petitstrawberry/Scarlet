fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("scarlet") {
        // Keep the interpreter away from the kernel's default PIE load bias.
        // Native std supplies its own `_start`; our assembly shim captures SP
        // before that startup has a chance to consume the initial stack.
        for argument in [
            "--entry=_scarlet_ld_start",
            "--image-base=0x40000000",
            "--no-dynamic-linker",
            "-static",
            "--no-pie",
            "-z",
            "max-page-size=4096",
        ] {
            println!("cargo:rustc-link-arg={argument}");
        }
    }
}
