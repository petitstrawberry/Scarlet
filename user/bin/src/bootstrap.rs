//! Bootstrap policy for the in-tree distributions, not a kernel ABI convention.

use std::{
    environment::{Environment, VfsView},
    format,
    fs::{self, File},
    handle::Handle,
    println,
    time::Duration,
    vec::Vec,
};

pub fn cmdline_value<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline
        .split_whitespace()
        .find_map(|word| word.strip_prefix(key))
}

fn directory(path: &str) -> Result<(), &'static str> {
    if fs::create_directory(path).is_ok() || fs::list_directory(path).is_ok() {
        Ok(())
    } else {
        Err("cannot create backing directory")
    }
}
fn view_directory(view: &VfsView, path: &str) -> Result<(), &'static str> {
    view.create_directory(path)
        .or_else(|_| view.open(path, 0).map(|_| ()))
        .map_err(|_| "cannot create view directory")
}
fn devfs() -> Result<(), &'static str> {
    directory("/dev")?;
    fs::mount("devfs", "/dev", "devfs", 0, None).map_err(|_| "cannot mount devfs")?;
    fs::mount("devpts", "/dev/pts", "devpts", 0, None).map_err(|_| "cannot mount devpts")
}
pub fn console() -> Result<[Handle; 3], &'static str> {
    devfs()?;
    let input = File::open("/dev/tty0")
        .map_err(|_| "cannot open console")?
        .into_handle();
    let output = input.duplicate().map_err(|_| "cannot duplicate console")?;
    let error = input.duplicate().map_err(|_| "cannot duplicate console")?;
    Ok([input, output, error])
}

/// Select the global root while still in bootstrap. Disk roots are writable
/// directly; diskless boot adds one volatile upper layer to the whole initramfs.
pub fn backing(cmdline: &str, require_disk: bool) -> Result<VfsView, &'static str> {
    directory("/mnt")?;
    directory("/mnt/newroot")?;
    let fstype = cmdline_value(cmdline, "rootfstype=").unwrap_or("ext2");
    let root = cmdline_value(cmdline, "root=");
    let rootwait = cmdline.split_whitespace().any(|word| word == "rootwait");
    let candidates = [root.unwrap_or("/dev/vblk0"), "/dev/usbblk0"];
    let mut attempts = 0u64;
    let mounted = 'retry: loop {
        attempts += 1;
        for (index, device) in candidates.iter().enumerate() {
            if index > 0 && root.is_some() {
                break;
            }
            // Avoid asking the filesystem to mount a device that has not yet
            // appeared while asynchronous device discovery is still running.
            if rootwait && File::open(device).is_err() {
                continue;
            }
            let options = format!("device={},rw", device);
            if fs::mount(device, "/mnt/newroot", fstype, 0, Some(&options)).is_ok() {
                if rootwait {
                    println!(
                        "init: rootwait: {} mounted after {} attempt(s)",
                        device, attempts
                    );
                }
                break 'retry true;
            }
        }
        if !rootwait {
            break false;
        }
        if attempts == 1 || attempts % 30 == 0 {
            println!(
                "init: rootwait: {} ({}) is not ready; retrying every second",
                root.unwrap_or("configured block device"),
                fstype
            );
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    if mounted {
        directory("/mnt/newroot/old_root")?;
        fs::pivot_root("/mnt/newroot", "/mnt/newroot/old_root")
            .map_err(|_| "cannot switch to backing disk")?;
        devfs()?;
    } else if require_disk || root.is_some() {
        return Err("configured root disk is unavailable");
    }
    let base = VfsView::current_admin().map_err(|_| "bootstrap view authority unavailable")?;
    let global = if mounted {
        base
    } else {
        // CpioFS is read-only. All ABI views share this writable global tree.
        let upper = VfsView::create("tmpfs", "size=128M")
            .map_err(|_| "cannot create volatile root storage")?;
        let global = VfsView::overlay(&base, "/", Some((&upper, "/")))
            .map_err(|_| "cannot construct volatile root view")?;
        for path in ["/dev", "/dev/pts"] {
            view_directory(&global, path)?;
            global
                .bind(path, &base, path)
                .map_err(|_| "cannot bind bootstrap devices")?;
        }
        global
    };
    for path in ["/home", "/shared", "/tmp"] {
        view_directory(&global, path)?;
    }
    global
        .mount("/tmp", "tmpfs", "size=128M")
        .map_err(|_| "cannot mount shared temporary storage")?;
    Ok(global)
}

fn abi_view(global: &VfsView, root: &str) -> Result<VfsView, &'static str> {
    let view = global
        .rooted_at(root)
        .map_err(|_| "cannot construct ABI root view")?;
    for (target, source) in [
        ("/dev", "/dev"),
        ("/dev/pts", "/dev/pts"),
        ("/tmp", "/tmp"),
        ("/home", "/home"),
        ("/shared", "/shared"),
        ("/root", "/root"),
        ("/scarlet", "/"),
    ] {
        view_directory(&view, target)?;
        view.bind(target, global, source)
            .map_err(|_| "cannot bind shared directory")?;
    }
    Ok(view)
}

/// Scarlet uses the global root; additional ABIs use roots under /systems.
/// Only additional ABI views expose the global tree at /scarlet.
pub fn environment(native: VfsView) -> Result<(Environment, Vec<VfsView>), &'static str> {
    let env = Environment::create().map_err(|_| "cannot create Environment")?;
    env.set_root("scarlet", &native)
        .map_err(|_| "cannot register Scarlet view")?;
    #[cfg(target_arch = "aarch64")]
    const OTHER_ABIS: &[&str] = &["linux-aarch64"];
    #[cfg(target_arch = "riscv64")]
    const OTHER_ABIS: &[&str] = &["linux-riscv64", "xv6-riscv64"];
    #[cfg(target_arch = "riscv32")]
    const OTHER_ABIS: &[&str] = &[];
    let mut views = Vec::new();
    for abi in OTHER_ABIS {
        let root = format!("/systems/{}", abi);
        if native.open(&root, 0).is_err() {
            continue;
        }
        let view = abi_view(&native, &root)?;
        env.set_root(abi, &view)
            .map_err(|_| "cannot register ABI view")?;
        views.push(view);
    }
    views.insert(0, native);
    env.seal().map_err(|_| "cannot seal Environment")?;
    Ok((env, views))
}
