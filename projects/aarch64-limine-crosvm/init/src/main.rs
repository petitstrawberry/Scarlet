#![no_std]
#![no_main]

extern crate scarlet_std as std;
#[path = "../../../../user/bin/src/bootstrap.rs"]
mod bootstrap;

use std::{environment::HandleMapping, println};

fn boot() -> Result<core::convert::Infallible, &'static str> {
    let stdio = bootstrap::console()?;
    let backing = bootstrap::backing("root=/dev/vblk1", true)?;
    let (environment, _views) = bootstrap::environment(backing)?;
    let linux = environment
        .root("linux-aarch64")
        .map_err(|_| "Linux view")?;
    let executable = linux
        .open("/usr/bin/crosvm", 0)
        .map_err(|_| "crosvm binary")?;
    let handles = [
        HandleMapping {
            source: &stdio[0],
            target: 0,
        },
        HandleMapping {
            source: &stdio[1],
            target: 1,
        },
        HandleMapping {
            source: &stdio[2],
            target: 2,
        },
    ];
    // One argument per line; use a separate target directory for each fixture.
    let configured_args: std::vec::Vec<&str> = option_env!("SCARLET_CROSVM_ARGS")
        .map(|args| args.lines().collect())
        .unwrap_or_default();
    let default_args = [
        "/usr/bin/crosvm",
        "--no-syslog",
        "--log-level",
        "debug",
        "run",
        "--disable-sandbox",
        "--no-pmu",
        "--no-rng",
        "--no-usb",
        "--cpus",
        "1",
        "--mem",
        "128",
        "--serial",
        "type=stdout,hardware=serial,num=1,console=true,stdin=false",
        "/guest/Image",
    ];
    let args = if configured_args.is_empty() {
        &default_args[..]
    } else {
        &configured_args[..]
    };
    println!("SCARLET_CROSVM_EXEC");
    environment
        .exec(
            &executable,
            args,
            &[
                "PATH=/usr/bin:/bin",
                "HOME=/root",
                "LD_LIBRARY_PATH=/lib:/usr/lib",
                "RUST_BACKTRACE=1",
            ],
            "/",
            &handles,
        )
        .map_err(|_| "crosvm exec")
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let Err(error) = boot();
    println!("SCARLET_CROSVM_FAIL: {}", error);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
