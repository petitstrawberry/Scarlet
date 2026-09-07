//! Launch Linux zathura with a document opened in the caller's own view.
//!
//! The document crosses the ABI boundary as standard input, not a rewritten
//! backing-filesystem path. Zathura's "-" input uses the explicitly mapped file.

#![no_std]
#![no_main]

extern crate scarlet_std as std;
mod abi_exec;

use std::string::String;
use std::vec::Vec;
use std::{fs, println};

const VIEWER_PATH: &str = "/usr/bin/zathura";
#[cfg(target_arch = "riscv64")]
const LINUX_ABI: &str = "linux-riscv64";
#[cfg(target_arch = "aarch64")]
const LINUX_ABI: &str = "linux-aarch64";

fn prepare_runtime_dirs() {
    let _ = fs::create_directory("/tmp/pdfview-zathura-config");
    let _ = fs::create_directory("/tmp/pdfview-zathura-data");
    let _ = fs::create_directory("/tmp/pdfview-zathura-cache");
    if let Ok(mut file) = fs::File::create("/tmp/pdfview-zathura-config/zathurarc") {
        let _ = file.write(b"set database null\n");
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() <= 1
        || args
            .last()
            .is_some_and(|arg| arg.starts_with('-') && arg != "-")
    {
        println!("usage: pdfview [zathura-options] <file.pdf|->");
        return 1;
    }
    let path = args.last().unwrap();
    let input = if path == "-" {
        None
    } else {
        match fs::File::open(path) {
            Ok(file) => Some(file.into_handle()),
            Err(error) => {
                println!("pdfview: cannot open {}: {}", path, error);
                return 1;
            }
        }
    };
    let mut argv = std::vec![
        VIEWER_PATH,
        "--config-dir=/tmp/pdfview-zathura-config",
        "--data-dir=/tmp/pdfview-zathura-data",
        "--cache-dir=/tmp/pdfview-zathura-cache",
        "--mode=presentation",
    ];
    argv.extend(args[1..args.len() - 1].iter().map(String::as_str));
    argv.push("-");
    prepare_runtime_dirs();
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
    ];
    let result = abi_exec::exec(LINUX_ABI, VIEWER_PATH, &argv, &envp, "/", input.as_ref());
    println!(
        "pdfview: failed to launch {} via {} ({:?})",
        VIEWER_PATH, LINUX_ABI, result
    );
    127
}
