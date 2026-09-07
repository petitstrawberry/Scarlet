//! Bootstrap policy for the in-tree distributions, not a kernel ABI convention.

use std::{
    environment::{Environment, VfsView},
    format,
    fs::{self, File},
    handle::Handle,
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

pub struct Backing {
    pub view: VfsView,
    disk_backed: bool,
}

/// Select backing storage while still in the bootstrap view. Disk roots are
/// writable directly; diskless boot keeps the read-only initramfs as lower data.
pub fn backing(cmdline: &str, require_disk: bool) -> Result<Backing, &'static str> {
    directory("/mnt")?;
    directory("/mnt/newroot")?;
    let fstype = cmdline_value(cmdline, "rootfstype=").unwrap_or("ext2");
    let root = cmdline_value(cmdline, "root=");
    let candidates = [root.unwrap_or("/dev/vblk0"), "/dev/usbblk0"];
    let mut mounted = false;
    for (index, device) in candidates.iter().enumerate() {
        if index > 0 && root.is_some() {
            break;
        }
        let options = format!("device={},rw", device);
        if fs::mount(device, "/mnt/newroot", fstype, 0, Some(&options)).is_ok() {
            mounted = true;
            break;
        }
    }
    if mounted {
        directory("/mnt/newroot/old_root")?;
        fs::pivot_root("/mnt/newroot", "/mnt/newroot/old_root")
            .map_err(|_| "cannot switch to backing disk")?;
        devfs()?;
    } else {
        if require_disk || root.is_some() {
            return Err("configured root disk is unavailable");
        }
        for path in ["/home", "/shared"] {
            directory(path)?;
            fs::mount("tmpfs", path, "tmpfs", 0, Some("size=128M"))
                .map_err(|_| "cannot mount volatile backing storage")?;
        }
    }
    for path in ["/home", "/shared", "/tmp"] {
        directory(path)?;
    }
    fs::mount("tmpfs", "/tmp", "tmpfs", 0, Some("size=128M"))
        .map_err(|_| "cannot mount shared temporary storage")?;
    Ok(Backing {
        view: VfsView::current_admin().map_err(|_| "bootstrap view authority unavailable")?,
        disk_backed: mounted,
    })
}

fn abi_view(base: &Backing, abi: &str) -> Result<VfsView, &'static str> {
    let root = format!("/systems/{}", abi);
    let view = if base.disk_backed {
        base.view
            .rooted_at(&root)
            .map_err(|_| "cannot construct ABI root view")?
    } else {
        // CpioFS is read-only. Keep this upper layer anonymous and volatile,
        // without a separate backing directory or persistent overlay layout.
        let upper = VfsView::create("tmpfs", "size=128M")
            .map_err(|_| "cannot create volatile ABI storage")?;
        VfsView::overlay(&base.view, &root, Some((&upper, "/")))
            .map_err(|_| "cannot construct volatile ABI view")?
    };
    for (target, source) in [
        ("/dev", "/dev"),
        ("/dev/pts", "/dev/pts"),
        ("/tmp", "/tmp"),
        ("/home", "/home"),
        ("/shared", "/shared"),
        ("/scarlet", "/"),
    ] {
        view_directory(&view, target)?;
        view.bind(target, &base.view, source)
            .map_err(|_| "cannot bind shared directory")?;
    }
    Ok(view)
}

/// Scarlet's default Environment. /scarlet is an ordinary, non-recursive
/// backing-root gateway chosen here. Custom/isolated environments can omit it.
pub fn environment(base: &Backing) -> Result<(Environment, Vec<VfsView>), &'static str> {
    let env = Environment::create().map_err(|_| "cannot create Environment")?;
    let native = abi_view(base, "scarlet")?;
    env.set_root("scarlet", &native)
        .map_err(|_| "cannot register Scarlet view")?;
    #[cfg(target_arch = "aarch64")]
    const OTHER_ABIS: &[&str] = &["linux-aarch64"];
    #[cfg(target_arch = "riscv64")]
    const OTHER_ABIS: &[&str] = &["linux-riscv64", "xv6-riscv64"];
    let mut views = Vec::new();
    for abi in OTHER_ABIS {
        if fs::list_directory(&format!("/systems/{}", abi)).is_err() {
            continue;
        }
        let view = abi_view(base, abi)?;
        // Keep the administrator's home coherent across native/Linux tools.
        view_directory(&view, "/root")?;
        view.bind("/root", &native, "/root")
            .map_err(|_| "cannot share root home")?;
        env.set_root(abi, &view)
            .map_err(|_| "cannot register ABI view")?;
        views.push(view);
    }
    views.insert(0, native);
    env.seal().map_err(|_| "cannot seal Environment")?;
    Ok((env, views))
}
