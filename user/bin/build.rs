fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("scarlet") {
        let aarch64 = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64");
        let ios_init = aarch64 && std::env::var_os("CARGO_FEATURE_IOS_INIT_PIE").is_some();
        // Bootstrap establishes the initial environment before it executes
        // clients that use scarlet-ld. The iOS catalog relocates its bundled
        // PIE before entry. Other bootstrap images use fixed placement.
        for binary in ["init", "microvm-init"] {
            let pie = if binary == "init" && ios_init {
                "-pie"
            } else {
                "-no-pie"
            };
            println!("cargo:rustc-link-arg-bin={binary}={pie}");
            println!("cargo:rustc-link-arg-bin={binary}=--no-dynamic-linker");
        }
        if ios_init || (aarch64 && std::env::var_os("CARGO_FEATURE_HOSTED_INIT_LAYOUT").is_some()) {
            // Both Darwin loaders use separate host pages for code and data.
            if !ios_init {
                // The process worker's bootstrap still uses a fixed address.
                println!("cargo:rustc-link-arg-bin=init=--image-base=0x7800000000");
            }
            for argument in [
                "-z",
                "max-page-size=16384",
                "-z",
                "separate-loadable-segments",
            ] {
                println!("cargo:rustc-link-arg-bin=init={argument}");
            }
        }
    }
}
