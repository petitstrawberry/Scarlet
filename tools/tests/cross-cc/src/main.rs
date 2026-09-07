//! Probe the actual cc-rs compiler selection under Cargo and the Nix dev shell.
//!
//! This unpublished, host-only fixture does not produce a Scarlet application.

use std::error::Error;
use std::path::Path;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [target, host, expected_compiler] = arguments.as_slice() else {
        return Err("usage: cross-cc-regression-test TARGET HOST EXPECTED_COMPILER".into());
    };

    let compiler = cc::Build::new()
        .host(host)
        .target(target)
        .opt_level(0)
        .debug(false)
        .cargo_metadata(false)
        .try_get_compiler()?;

    if compiler.path() != Path::new(expected_compiler) {
        return Err(format!(
            "{target}: expected compiler {expected_compiler}, selected {}",
            compiler.path().display()
        )
        .into());
    }

    println!("{target}: {}", compiler.path().display());
    Ok(())
}
