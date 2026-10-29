fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("scarlet") {
        for arg in ["--hash-style=both", "-z", "now", "--entry=0", "--exclude-libs=ALL", "-soname", "libsws_client_c.so", "--unresolved-symbols=ignore-all"] {
            println!("cargo::rustc-link-arg-cdylib={arg}");
        }
    }
}
