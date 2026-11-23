use scarlet::{
    DeviceSource,
    device::{
        Device,
        manager::{DeviceManager, DriverPriority},
    },
    hosted,
};
use scarlet_hosted_runtime::signed_image::Image;
use std::{
    cell::RefCell,
    ffi::{CStr, OsStr, c_char},
    io::{self, Read, Write},
    os::unix::{ffi::OsStrExt, net::UnixStream},
    path::Path,
    sync::{Arc, OnceLock, mpsc},
    thread,
};

use crate::{Log, block::RootBlock, log, root_image::RootImage, terminal::HostTerminal};

const CONSOLE_INPUT_LIMIT: usize = 4096;
static CONSOLE_INPUT: OnceLock<mpsc::SyncSender<Vec<u8>>> = OnceLock::new();

thread_local! {
    static CONSOLE: RefCell<Option<(UnixStream, Log)>> = const { RefCell::new(None) };
    static ROOT: RefCell<Option<RootImage>> = const { RefCell::new(None) };
    static TERMINAL: RefCell<Option<HostTerminal>> = const { RefCell::new(None) };
}

fn populate(manager: &DeviceManager, priority: DriverPriority) {
    if priority != DriverPriority::Core {
        return;
    }
    let Some((stream, callback)) = CONSOLE.take() else {
        return;
    };
    match HostTerminal::attach_stream(stream, 0, 17, move |result| {
        if let Err(error) = result {
            log(callback, &format!("Scarlet console stopped: {error}"));
        }
    }) {
        Ok(terminal) => TERMINAL.set(Some(terminal)),
        Err(error) => log(callback, &format!("Cannot attach Scarlet console: {error}")),
    }
    if let Some(image) = ROOT.take() {
        match RootBlock::attach(image, 0, 16) {
            Ok(block) => {
                manager.register_device_with_name(block.name().into(), Arc::new(block));
            }
            Err(error) => log(
                callback,
                &format!("SCARLET_BOOT_FAIL: Cannot attach root: {error}"),
            ),
        }
    }
}

fn forward_console(mut stream: UnixStream, screen: Log) -> io::Result<()> {
    let mut terminal = vt100::Parser::new(24, 80, 0);
    let mut bytes = [0; 4096];
    loop {
        let length = stream.read(&mut bytes)?;
        if length == 0 {
            return Ok(());
        }
        let bytes = &bytes[..length];
        io::stderr().write_all(bytes)?;
        terminal.process(bytes);
        log(screen, &terminal.screen().contents());
    }
}

fn boot(
    bundle: &Path,
    root: &Path,
    callback: Log,
    screen: Log,
    boot_check: bool,
) -> io::Result<()> {
    for name in ["ScarletInit", "ScarletBootCheck", "ScarletShell"] {
        let path = bundle
            .join("Frameworks")
            .join(format!("{name}.framework/{name}"));
        // SAFETY: The fixed path names refer to signed, immutable app bundle resources.
        let image = unsafe { Image::open(&path) }
            .map_err(|error| io::Error::other(format!("{name}: {error}")))?;
        hosted::register_signed_image(Arc::new(image)).map_err(io::Error::other)?;
    }
    let initramfs = std::fs::read(bundle.join("initramfs.cpio"))?;
    if initramfs.is_empty() {
        return Err(io::Error::other("Empty Scarlet initramfs"));
    }
    let root = RootImage::open(root)?;
    let platform =
        hosted::prepare(hosted::DEFAULT_VCPUS, 64 * 1024 * 1024).map_err(io::Error::other)?;
    let (guest, host) = UnixStream::pair()?;
    let mut writer = host.try_clone()?;
    let (send, receive) = mpsc::sync_channel::<Vec<u8>>(16);
    thread::Builder::new()
        .name("scarlet-console-input".into())
        .spawn(move || {
            for bytes in receive {
                if let Err(error) = writer.write_all(&bytes) {
                    log(callback, &format!("Cannot write Scarlet console: {error}"));
                    break;
                }
            }
        })?;
    thread::Builder::new()
        .name("scarlet-console-output".into())
        .spawn(move || {
            if let Err(error) = forward_console(host, screen) {
                log(callback, &format!("Cannot read Scarlet console: {error}"));
            }
        })?;
    CONSOLE_INPUT
        .set(send)
        .map_err(|_| io::Error::other("Scarlet console input is already running"))?;
    CONSOLE.set(Some((guest, callback)));
    ROOT.set(Some(root));
    platform.boot(
        &initramfs,
        if boot_check {
            "root=/dev/hostroot rootfstype=ext2 init.exec=/old_root/bin/boot-check"
        } else {
            "root=/dev/hostroot rootfstype=ext2 init.exec=/old_root/bin/sh"
        },
        DeviceSource::Hosted(populate),
    )
}

/// Queue bytes for the host I/O thread without blocking UIKit.
///
/// # Safety
///
/// `bytes` must be readable for `length` bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_console_send(bytes: *const u8, length: usize) -> bool {
    let Some(input) = CONSOLE_INPUT.get() else {
        return false;
    };
    if bytes.is_null() || length == 0 || length > CONSOLE_INPUT_LIMIT {
        return false;
    }
    let data = unsafe { std::slice::from_raw_parts(bytes, length) };
    input.try_send(data.to_vec()).is_ok()
}

/// # Safety
///
/// `bundle` and `root` are NUL-terminated paths to the running app bundle and
/// its writable root image. The callback remains callable from any host thread
/// for the lifetime of the app.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_boot(
    bundle: *const c_char,
    root: *const c_char,
    boot_check: bool,
    callback: Log,
    screen: Log,
) {
    let bundle = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(bundle) }.to_bytes(),
    ));
    let root = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(root) }.to_bytes(),
    ));
    if let Err(error) = boot(bundle, root, callback, screen, boot_check) {
        log(callback, &format!("SCARLET_BOOT_FAIL: {error}"));
    }
}
