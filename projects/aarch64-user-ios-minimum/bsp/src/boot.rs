use scarlet::{
    DeviceSource,
    device::{
        Device,
        manager::{DeviceManager, DriverPriority},
    },
    fs::{
        FileSystemError, FileSystemErrorKind, FileSystemOperations, FileType,
        drivers::ext2::Ext2FileSystem,
    },
    hosted,
};
use scarlet_hosted_runtime::signed_image::Image;
use std::{
    cell::RefCell,
    ffi::{CStr, CString, OsStr, c_char},
    io::{self, Read, Write},
    os::unix::{ffi::OsStrExt, net::UnixStream},
    path::Path,
    sync::{Arc, OnceLock, mpsc},
    thread,
};

use crate::{Log, block::RootBlock, display, log, root_image::RootImage, terminal::HostTerminal};

const CONSOLE_INPUT_LIMIT: usize = 4096;
static CONSOLE_INPUT: OnceLock<mpsc::SyncSender<Vec<u8>>> = OnceLock::new();

#[repr(C)]
pub struct TerminalState {
    row: u16,
    column: u16,
    cursor_visible: bool,
    application_cursor: bool,
    bracketed_paste: bool,
}

type Screen = extern "C" fn(*const c_char, TerminalState);

thread_local! {
    static CONSOLE: RefCell<Option<(UnixStream, Log)>> = const { RefCell::new(None) };
    static DISPLAY: RefCell<Option<display::Configuration>> = const { RefCell::new(None) };
    static ROOT: RefCell<Option<RootImage>> = const { RefCell::new(None) };
    static TERMINAL: RefCell<Option<HostTerminal>> = const { RefCell::new(None) };
}

// Earlier root images contain only the writable data directories. Install the
// distribution paths on that volume before the guest mounts it. Existing
// account configuration and installed binaries belong to the guest.
fn prepare_distribution(block: Arc<RootBlock>) -> Result<(), FileSystemError> {
    let fs = Ext2FileSystem::new(block)?;
    let root = fs.root_node();
    let missing = |name: &str| match fs.lookup(&root, &name.into()) {
        Ok(_) => Ok(false),
        Err(error) if error.kind == FileSystemErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    };
    if missing("lib")? {
        fs.create(
            &root,
            &"lib".into(),
            FileType::SymbolicLink("/old_root/lib".into()),
            0o777,
        )?;
    }
    if !missing("bin")? {
        return Ok(());
    }
    if missing("etc")? {
        // Publish the complete factory configuration in one directory rename.
        // A previous attempt may have stopped before publication.
        let stage = ".scarlet-factory-etc".to_string();
        let dir = if missing(&stage)? {
            fs.create(&root, &stage, FileType::Directory, 0o755)?
        } else {
            fs.lookup(&root, &stage)?
        };
        let name = "scarlet-users".to_string();
        let node = match fs.lookup(&dir, &name) {
            Ok(node) => node,
            Err(error) if error.kind == FileSystemErrorKind::NotFound => {
                fs.create(&dir, &name, FileType::RegularFile, 0o600)?
            }
            Err(error) => return Err(error),
        };
        let file = fs.open(&node, 2)?;
        let record = b"root:0:0::\n";
        let io_error =
            || FileSystemError::new(FileSystemErrorKind::IoError, "Cannot seed console accounts");
        if file.write_at(0, record).ok() != Some(record.len()) {
            return Err(io_error());
        }
        file.sync().map_err(|_| io_error())?;
        fs.rename(&root, &stage, &root, &"etc".into())?;
    }
    fs.create(
        &root,
        &"bin".into(),
        FileType::SymbolicLink("/old_root/bin".into()),
        0o777,
    )?;
    Ok(())
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
                let block = Arc::new(block);
                if let Err(error) = prepare_distribution(block.clone()) {
                    log(
                        callback,
                        &format!("SCARLET_BOOT_FAIL: root setup: {error:?}"),
                    );
                    return;
                }
                manager.register_device_with_name(block.name().into(), block);
            }
            Err(error) => log(
                callback,
                &format!("SCARLET_BOOT_FAIL: Cannot attach root: {error}"),
            ),
        }
    }
    if let Some(display) = DISPLAY.take()
        && let Err(error) = display.register(manager)
    {
        log(callback, &format!("Cannot register iOS display: {error}"));
    }
}

fn forward_console(mut stream: UnixStream, callback: Screen) -> io::Result<()> {
    let mut terminal = vt100::Parser::new(25, 80, 0);
    let mut bytes = [0; 4096];
    loop {
        let length = stream.read(&mut bytes)?;
        if length == 0 {
            return Ok(());
        }
        let bytes = &bytes[..length];
        io::stderr().write_all(bytes)?;
        terminal.process(bytes);
        let screen = terminal.screen();
        let (_, columns) = screen.size();
        let rows = screen.rows(0, columns).collect::<Vec<_>>().join("\n");
        let (row, column) = screen.cursor_position();
        let state = TerminalState {
            row,
            column,
            cursor_visible: !screen.hide_cursor(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
        };
        if let Ok(rows) = CString::new(rows) {
            callback(rows.as_ptr(), state);
        }
    }
}

fn boot(
    bundle: &Path,
    root: &Path,
    callback: Log,
    screen: Screen,
    boot_check: bool,
    login: bool,
    shell: bool,
    display: display::Configuration,
) -> io::Result<()> {
    let catalog = std::fs::read_to_string(bundle.join("guest-images"))?;
    for name in catalog.lines() {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(io::Error::other("Invalid bundle image name"));
        }
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
    crate::network::configure(bundle);
    // A native-resolution desktop shares guest RAM with applications and
    // their compositor buffers.
    let ram_mib = if display.width != 0 && display.height != 0 {
        2048
    } else {
        256
    };
    let platform = hosted::prepare(hosted::DEFAULT_VCPUS, ram_mib * 1024 * 1024)
        .map_err(io::Error::other)?;
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
    DISPLAY.set(Some(display));
    platform.boot(
        &initramfs,
        if boot_check {
            "root=/dev/hostroot rootfstype=ext2 init.exec=/old_root/bin/boot-check"
        } else if login {
            "root=/dev/hostroot rootfstype=ext2 init.exec=/bin/login"
        } else if shell {
            "root=/dev/hostroot rootfstype=ext2 init.exec=/bin/sh"
        } else {
            "root=/dev/hostroot rootfstype=ext2"
        },
        DeviceSource::Hosted(populate),
    )
}

/// Queue bytes for the host I/O thread without blocking the keyboard.
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
    login: bool,
    shell: bool,
    display: display::Configuration,
    callback: Log,
    screen: Screen,
) {
    let bundle = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(bundle) }.to_bytes(),
    ));
    let root = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(root) }.to_bytes(),
    ));
    if let Err(error) = boot(
        bundle, root, callback, screen, boot_check, login, shell, display,
    ) {
        log(callback, &format!("SCARLET_BOOT_FAIL: {error}"));
    }
}
