use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo::metadata=include={}", root.join("include").display());
    println!("cargo::metadata=native={}", root.join("native").display());
    println!("cargo::rerun-if-changed=build.rs");
}
