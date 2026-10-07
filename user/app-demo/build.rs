use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=app.toml");
    let recipe = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("app.toml");
    let metadata: toml::Value = toml::from_str(&fs::read_to_string(recipe).unwrap()).unwrap();
    for (key, variable) in [("id", "APP_ID"), ("name", "APP_NAME")] {
        let value = metadata["app"][key].as_str().expect("application metadata");
        println!("cargo:rustc-env={variable}={value}");
    }
}
