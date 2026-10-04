fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("scarlet") {
        // Bootstrap establishes the initial environment before it executes
        // clients that use scarlet-ld. Keep that startup independent of the
        // shared userspace configuration's dynamic linker input. The kernel
        // maps ELF segments but leaves relocations to scarlet-ld, so merely
        // removing PT_INTERP from a PIE would leave its pointers unrelocated.
        for binary in ["init", "microvm-init"] {
            println!("cargo:rustc-link-arg-bin={binary}=-no-pie");
            println!("cargo:rustc-link-arg-bin={binary}=--no-dynamic-linker");
        }
    }
}
