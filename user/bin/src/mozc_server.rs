//! Launch the Linux ABI Mozc server.
//!
//! The actual conversion engine is the Linux `mozc_server` binary. This
//! Scarlet-native launcher selects the Linux ABI explicitly and supplies the
//! runtime environment used by `scarlet-mozc`.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::string::String;
use std::task::{EXECVE_FORCE_ABI_REBUILD, execve_abi_with_flags};
use std::vec::Vec;
use std::{env, fs, println};

#[cfg(target_arch = "aarch64")]
const SERVER_PATH: &str = "/scarlet/system/linux-aarch64/usr/lib/mozc/mozc_server";
#[cfg(target_arch = "aarch64")]
const SERVER_ARG0: &str = "/usr/lib/mozc/mozc_server";
#[cfg(target_arch = "aarch64")]
const LINUX_ABI: &str = "linux-aarch64";

#[cfg(target_arch = "riscv64")]
const SERVER_PATH: &str = "/scarlet/system/linux-riscv64/usr/lib/mozc/mozc_server";
#[cfg(target_arch = "riscv64")]
const SERVER_ARG0: &str = "/usr/lib/mozc/mozc_server";
#[cfg(target_arch = "riscv64")]
const LINUX_ABI: &str = "linux-riscv64";

const MOZC_PROFILE_DIR: &str = "/scarlet/system/scarlet/root/.config/mozc";

#[unsafe(no_mangle)]
fn main() -> i32 {
    ensure_mozc_profile_dir();

    let args: Vec<String> = env::args().collect();
    let mut server_args: Vec<String> = Vec::new();
    server_args.push(String::from(SERVER_ARG0));
    server_args.extend(args.iter().skip(1).cloned());

    let argv: Vec<&str> = server_args.iter().map(|arg| arg.as_str()).collect();
    let envp = [
        "LD_LIBRARY_PATH=/usr/lib:/lib",
        "PATH=/bin:/usr/bin",
        "HOME=/scarlet/system/scarlet/root",
        "XDG_CONFIG_HOME=/scarlet/system/scarlet/root/.config",
        "TMPDIR=/tmp",
        "XDG_RUNTIME_DIR=/tmp",
    ];

    let result = execve_abi_with_flags(
        SERVER_PATH,
        &argv,
        &envp,
        LINUX_ABI,
        EXECVE_FORCE_ABI_REBUILD,
    );

    println!(
        "mozc-server: failed to launch {} via {} (rc={})",
        SERVER_PATH, LINUX_ABI, result
    );
    127
}

fn ensure_mozc_profile_dir() {
    let _ = fs::create_directory("/scarlet/system/scarlet/root/.config");
    let _ = fs::create_directory(MOZC_PROFILE_DIR);
}
