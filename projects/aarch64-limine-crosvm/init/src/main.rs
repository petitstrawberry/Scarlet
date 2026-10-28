#![no_std]
#![no_main]

extern crate scarlet_std as std;
#[path = "../../../../user/bin/src/bootstrap.rs"]
mod bootstrap;

use std::{environment::HandleMapping, println};

fn boot(stdio: &[std::handle::Handle; 3]) -> Result<core::convert::Infallible, &'static str> {
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
    let env = [
        "PATH=/usr/bin:/bin",
        "HOME=/root",
        "LD_LIBRARY_PATH=/lib:/usr/lib",
        "RUST_BACKTRACE=1",
    ];
    if option_env!("SCARLET_CROSVM_KERNEL_LOG") == Some("1") {
        // Keep a native parent to forward the host kernel ring while crosvm
        // runs. This diagnoses failures before crosvm's own logger starts.
        let mut log = std::fs::File::open("/dev/kmsg").map_err(|_| "kernel log")?;
        environment
            .spawn(&executable, args, &env, "/", &handles)
            .map_err(|_| "crosvm spawn")?;
        let mut bytes = [0; 8192];
        loop {
            let count = log.read(&mut bytes).map_err(|_| "kernel log read")?;
            if count == 0 {
                return Err("kernel log ended");
            }
            std::print!("{}", std::string::String::from_utf8_lossy(&bytes[..count]));
        }
    }
    environment
        .exec(&executable, args, &env, "/", &handles)
        .map_err(|_| "crosvm exec")
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    // Keep standard handles alive when boot returns an error so the host can
    // actually receive the failure marker.
    let stdio = match bootstrap::console() {
        Ok(handles) => handles,
        Err(_) => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
    };
    let Err(error) = boot(&stdio);
    println!("SCARLET_CROSVM_FAIL: {}", error);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
