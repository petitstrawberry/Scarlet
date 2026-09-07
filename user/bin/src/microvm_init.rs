#![no_std]
#![no_main]

extern crate scarlet_std as std;
mod bootstrap;

use std::{environment::HandleMapping, println};

fn boot() -> Result<core::convert::Infallible, &'static str> {
    let stdio = bootstrap::console()?;
    let args = std::env::args_vec();
    let cmdline = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let backing = bootstrap::backing(cmdline, true)?;
    let (environment, _views) = bootstrap::environment(backing)?;
    let linux = environment
        .root("linux-aarch64")
        .map_err(|_| "Linux view is unavailable")?;
    let executable = linux
        .open("/usr/bin/firecracker", 0)
        .map_err(|_| "missing firecracker")?;
    for path in [
        "/usr/bin/guest-Image",
        "/usr/bin/guest-initramfs.cpio.gz",
        "/etc/firecracker/scarlet-microvm-aarch64.json",
    ] {
        linux
            .open(path, 0)
            .map_err(|_| "missing microvm guest artifact")?;
    }
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
    environment
        .exec(
            &executable,
            &[
                "/usr/bin/firecracker",
                "--no-api",
                "--no-seccomp",
                "--config-file",
                "/etc/firecracker/scarlet-microvm-aarch64.json",
            ],
            &[
                "PATH=/bin:/usr/bin",
                "LD_LIBRARY_PATH=/usr/lib:/lib",
                "HOME=/root",
            ],
            "/",
            &handles,
        )
        .map_err(|_| "Environment exec of firecracker failed")
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    if let Err(error) = boot() {
        println!("microvm-init: {}", error);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
