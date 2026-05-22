//! Launch the Linux/Wayland PDF viewer.
//!
//! The renderer is the Linux zathura package built by `tools/linux/build_user_programs.sh`.
//! This Scarlet-native launcher only translates Scarlet-visible paths and starts
//! zathura with the environment expected by the Linux rootfs.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::string::{String, ToString};
use std::task::{EXECVE_FORCE_ABI_REBUILD, execve_abi_with_flags};
use std::vec::Vec;
use std::{format, println};

#[cfg(target_arch = "riscv64")]
const VIEWER_PATH: &str = "/scarlet/system/linux-riscv64/usr/bin/zathura";
#[cfg(target_arch = "aarch64")]
const VIEWER_PATH: &str = "/scarlet/system/linux-aarch64/usr/bin/zathura";

#[cfg(target_arch = "riscv64")]
const LINUX_ABI: &str = "linux-riscv64";
#[cfg(target_arch = "aarch64")]
const LINUX_ABI: &str = "linux-aarch64";

#[cfg(target_arch = "riscv64")]
const LINUX_SYSTEM_PREFIX: &str = "/system/linux-riscv64";
#[cfg(target_arch = "aarch64")]
const LINUX_SYSTEM_PREFIX: &str = "/system/linux-aarch64";

fn print_usage() {
    println!("usage: pdfview [zathura-options] <file.pdf>");
    println!("       requires {}", VIEWER_PATH);
}

fn linux_visible_path(path: &str) -> String {
    if path.starts_with("/scarlet/")
        || path == "/scarlet"
        || path.starts_with("/home/")
        || path == "/home"
        || path.starts_with("/tmp/")
        || path == "/tmp"
        || path.starts_with("/dev/")
        || path == "/dev"
        || path.starts_with("/data/shared/")
        || path == "/data/shared"
        || !path.starts_with('/')
    {
        return path.to_string();
    }

    if path == LINUX_SYSTEM_PREFIX {
        return String::from("/");
    }
    if path.starts_with(LINUX_SYSTEM_PREFIX)
        && path.as_bytes().get(LINUX_SYSTEM_PREFIX.len()) == Some(&b'/')
    {
        return path[LINUX_SYSTEM_PREFIX.len()..].to_string();
    }

    format!("/scarlet/system/scarlet{}", path)
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() <= 1 {
        print_usage();
        return 1;
    }

    let mut viewer_args: Vec<String> = Vec::new();
    viewer_args.push(String::from("/usr/bin/zathura"));

    let mut saw_pdf_path = false;
    for arg in args.iter().skip(1) {
        if arg.starts_with('-') {
            viewer_args.push(arg.clone());
        } else {
            viewer_args.push(linux_visible_path(arg));
            saw_pdf_path = true;
        }
    }

    if !saw_pdf_path {
        print_usage();
        return 1;
    }

    let argv: Vec<&str> = viewer_args.iter().map(|arg| arg.as_str()).collect();
    let envp = [
        "LD_LIBRARY_PATH=/usr/lib:/lib",
        "PATH=/bin:/usr/bin",
        "HOME=/root",
        "GDK_BACKEND=wayland",
        "NO_AT_BRIDGE=1",
        "GTK_USE_PORTAL=0",
        "MESA_LOADER_DRIVER_OVERRIDE=swrast",
        "WAYLAND_DISPLAY=wayland-0",
        "XDG_RUNTIME_DIR=/tmp",
        "XDG_DATA_DIRS=/usr/share",
        "ZATHURA_PLUGIN_PATH=/usr/lib/zathura",
    ];

    let result = execve_abi_with_flags(
        VIEWER_PATH,
        &argv,
        &envp,
        LINUX_ABI,
        EXECVE_FORCE_ABI_REBUILD,
    );

    println!(
        "pdfview: failed to launch {} via {} (rc={})",
        VIEWER_PATH, LINUX_ABI, result
    );
    127
}
