#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    env, format,
    fs::{File, create_directory, mount, pivot_root},
    handle::Handle,
    println,
    task::{EXECVE_FORCE_ABI_REBUILD, execve_with_flags, getpid},
};

static mut STDIN: Option<Handle> = None;
static mut STDOUT: Option<Handle> = None;
static mut STDERR: Option<Handle> = None;

const DEFAULT_ROOT_DEVICE: &str = "/dev/vblk0";
const DEFAULT_ROOT_FSTYPE: &str = "ext2";

fn cmdline_value<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    for token in cmdline.split_whitespace() {
        if let Some(value) = token.strip_prefix(key) {
            return Some(value);
        }
    }
    None
}

fn setup_devfs() -> Result<(), &'static str> {
    let _ = create_directory("/dev");
    mount("devfs", "/dev", "devfs", 0, None).map_err(|_| "failed to mount devfs")?;
    match mount("devpts", "/dev/pts", "devpts", 0, None) {
        Ok(_) => println!("microvm_init: devpts mounted at /dev/pts"),
        Err(error) => println!("microvm_init: Warning: failed to mount devpts: {}", error),
    }
    Ok(())
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

fn mount_rootfs(cmdline: &str) -> Result<(), &'static str> {
    let _ = create_directory("/mnt");
    let _ = create_directory("/mnt/newroot");

    let root_device = cmdline_value(cmdline, "root=").unwrap_or(DEFAULT_ROOT_DEVICE);
    let root_fstype = cmdline_value(cmdline, "rootfstype=").unwrap_or(DEFAULT_ROOT_FSTYPE);
    let root_options = format!("device={},rw", root_device);
    mount(
        root_device,
        "/mnt/newroot",
        root_fstype,
        0,
        Some(&root_options),
    )
    .map_err(|_| "failed to mount configured rootfs")?;

    let _ = create_directory("/mnt/newroot/tmp");
    let _ = mount("tmpfs", "/mnt/newroot/tmp", "tmpfs", 0, Some("size=32M"));

    let _ = create_directory("/mnt/newroot/old_root");
    pivot_root("/mnt/newroot", "/mnt/newroot/old_root").map_err(|_| "pivot_root failed")
}

const FIRECRACKER: &str = "/system/linux-aarch64/usr/bin/firecracker";
const FIRECRACKER_CONFIG: &str = "/etc/firecracker/scarlet-microvm-aarch64.json";
const GUEST_KERNEL: &str = "/system/linux-aarch64/usr/bin/guest-Image";
const GUEST_INITRAMFS: &str = "/system/linux-aarch64/usr/bin/guest-initramfs.cpio.gz";

fn verify_guest_artifacts() -> Result<(), &'static str> {
    File::open(FIRECRACKER).map_err(|_| "missing /usr/bin/firecracker in linux-aarch64 rootfs")?;
    File::open(GUEST_KERNEL).map_err(|_| "missing /usr/bin/guest-Image in linux-aarch64 rootfs")?;
    File::open(GUEST_INITRAMFS)
        .map_err(|_| "missing /usr/bin/guest-initramfs.cpio.gz in linux-aarch64 rootfs")?;
    Ok(())
}

fn exec_firecracker() -> ! {
    let argv = [
        FIRECRACKER,
        "--no-api",
        "--no-seccomp",
        "--config-file",
        FIRECRACKER_CONFIG,
    ];
    let envp = [
        "LD_LIBRARY_PATH=/usr/lib:/lib",
        "PATH=/bin:/usr/bin:/scarlet/system/scarlet/bin",
    ];

    println!("microvm-init: exec {}", FIRECRACKER);
    let _ = execve_with_flags(FIRECRACKER, &argv, &envp, EXECVE_FORCE_ABI_REBUILD);
    println!("microvm-init: failed to exec {}", FIRECRACKER);
    loop {}
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = env::args_vec();
    let cmdline = args.get(1).map(|arg| arg.as_str()).unwrap_or("");

    if let Err(error) = setup_devfs() {
        println!("microvm-init: {}", error);
        return -1;
    }
    if let Err(error) = setup_stdio() {
        println!("microvm-init: {}", error);
        return -1;
    }

    println!("microvm-init: pid={}", getpid());
    if !cmdline.is_empty() {
        println!("microvm-init: boot cmdline: {}", cmdline);
    }

    if let Err(error) = mount_rootfs(cmdline) {
        println!("microvm-init: {}", error);
        return -1;
    }

    if let Err(error) = setup_devfs() {
        println!("microvm-init: {}", error);
        return -1;
    }

    if let Err(error) = verify_guest_artifacts() {
        println!("microvm-init: {}", error);
        return -1;
    }

    exec_firecracker();
}
