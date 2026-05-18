#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    fs::{File, create_directory, mount, pivot_root},
    handle::Handle,
    println,
    task::{EXECVE_FORCE_ABI_REBUILD, execve_with_flags, getpid},
};

static mut STDIN: Option<Handle> = None;
static mut STDOUT: Option<Handle> = None;
static mut STDERR: Option<Handle> = None;

fn setup_devfs() -> Result<(), &'static str> {
    let _ = create_directory("/dev");
    mount("devfs", "/dev", "devfs", 0, None).map_err(|_| "failed to mount devfs")
}

fn setup_stdio() -> Result<(), &'static str> {
    let tty = File::open("/dev/tty0").map_err(|_| "failed to open /dev/tty0")?;
    let stdin = tty.into_handle();
    let stdout = stdin
        .duplicate()
        .map_err(|_| "failed to duplicate stdout")?;
    let stderr = stdin
        .duplicate()
        .map_err(|_| "failed to duplicate stderr")?;

    unsafe {
        STDIN = Some(stdin);
        STDOUT = Some(stdout);
        STDERR = Some(stderr);
    }
    Ok(())
}

fn mount_rootfs() -> Result<(), &'static str> {
    let _ = create_directory("/mnt");
    let _ = create_directory("/mnt/newroot");

    mount(
        "/dev/vblk0",
        "/mnt/newroot",
        "ext2",
        0,
        Some("device=/dev/vblk0,rw"),
    )
    .map_err(|_| "failed to mount /dev/vblk0 as ext2")?;

    let _ = create_directory("/mnt/newroot/tmp");
    let _ = mount("tmpfs", "/mnt/newroot/tmp", "tmpfs", 0, Some("size=32M"));

    let _ = create_directory("/mnt/newroot/old_root");
    pivot_root("/mnt/newroot", "/mnt/newroot/old_root").map_err(|_| "pivot_root failed")
}

fn exec_firecracker() -> ! {
    let firecracker = "/system/linux-aarch64/usr/bin/firecracker";
    let config = "/scarlet/system/linux-aarch64/etc/firecracker/unikraft-helloworld-aarch64.json";
    let argv = [
        firecracker,
        "--no-api",
        "--no-seccomp",
        "--config-file",
        config,
    ];
    let envp = [
        "LD_LIBRARY_PATH=/scarlet/system/linux-aarch64/usr/lib:/scarlet/system/linux-aarch64/lib",
        "PATH=/scarlet/system/linux-aarch64/bin:/scarlet/system/linux-aarch64/usr/bin:/scarlet/system/scarlet/bin",
    ];

    println!("microvm-init: exec {}", firecracker);
    let _ = execve_with_flags(firecracker, &argv, &envp, EXECVE_FORCE_ABI_REBUILD);
    println!("microvm-init: failed to exec {}", firecracker);
    loop {}
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    if let Err(error) = setup_devfs() {
        println!("microvm-init: {}", error);
        return -1;
    }
    if let Err(error) = setup_stdio() {
        println!("microvm-init: {}", error);
        return -1;
    }

    println!("microvm-init: pid={}", getpid());

    if let Err(error) = mount_rootfs() {
        println!("microvm-init: {}", error);
        return -1;
    }

    if let Err(error) = setup_devfs() {
        println!("microvm-init: {}", error);
        return -1;
    }

    exec_firecracker();
}
