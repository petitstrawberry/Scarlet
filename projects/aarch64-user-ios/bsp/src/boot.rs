use scarlet::{
    DeviceSource,
    device::manager::{DeviceManager, DriverPriority},
    hosted,
};
use scarlet_hosted_runtime::signed_image::Image;
use std::{
    cell::RefCell,
    ffi::{CStr, OsStr, c_char},
    io::{self, BufRead, BufReader, Read},
    os::unix::{ffi::OsStrExt, net::UnixStream},
    path::Path,
    sync::Arc,
    thread,
};

use crate::{Log, log, terminal::HostTerminal};

thread_local! {
    static CONSOLE: RefCell<Option<(UnixStream, Log)>> = const { RefCell::new(None) };
    static TERMINAL: RefCell<Option<HostTerminal>> = const { RefCell::new(None) };
}

fn populate(_: &DeviceManager, priority: DriverPriority) {
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
}

fn forward_console(stream: UnixStream, callback: Log) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    while (&mut reader).take(4096).read_until(b'\n', &mut line)? != 0 {
        let message = String::from_utf8_lossy(&line)
            .trim_end_matches(['\r', '\n'])
            .replace('\0', "\u{fffd}");
        log(callback, &message);
        line.clear();
    }
    Ok(())
}

fn boot(bundle: &Path, callback: Log) -> io::Result<()> {
    for name in ["ScarletInit", "ScarletBootCheck"] {
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
    let platform =
        hosted::prepare(hosted::DEFAULT_VCPUS, 64 * 1024 * 1024).map_err(io::Error::other)?;
    let (guest, host) = UnixStream::pair()?;
    thread::Builder::new()
        .name("scarlet-console-output".into())
        .spawn(move || {
            if let Err(error) = forward_console(host, callback) {
                log(callback, &format!("Cannot read Scarlet console: {error}"));
            }
        })?;
    CONSOLE.set(Some((guest, callback)));
    platform.boot(
        &initramfs,
        "init.exec=/bin/boot-check",
        DeviceSource::Hosted(populate),
    )
}

/// # Safety
///
/// `bundle` is a NUL-terminated path to the running app bundle. The callback
/// remains callable from any host thread for the lifetime of the app.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_ios_boot(bundle: *const c_char, callback: Log) {
    let bundle = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(bundle) }.to_bytes(),
    ));
    if let Err(error) = boot(bundle, callback) {
        log(callback, &format!("SCARLET_BOOT_FAIL: {error}"));
    }
}
