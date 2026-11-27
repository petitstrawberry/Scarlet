#[cfg(not(any(target_os = "scarlet", test)))]
compile_error!("btctl requires a Scarlet Native target");

use scarlet_abi::{bluetooth as bt, fs as vfs};
use scarlet_os::Handle;
use scarlet_sys::{Syscall, syscall1, syscall2, syscall3, syscall4};
use std::ffi::{CStr, CString};
use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

const USAGE: &str =
    "usage: btctl [--device PATH | /dev/PATH] status|scan|pair MAC|connect MAC|disconnect";
const DEFAULT_DEVICE: &str = "/dev/bluetooth0";
type Result<T> = std::result::Result<T, String>;

fn parse_address(text: &str) -> Result<bt::Address> {
    let mut address = [0; 6];
    let mut parts = text.split(':');
    for byte in address.iter_mut().rev() {
        let part = parts.next().unwrap_or_default();
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("expected a MAC in AA:BB:CC:DD:EE:FF form".into());
        }
        *byte = u8::from_str_radix(part, 16).unwrap();
    }
    if parts.next().is_some() || address == [0; 6] || address == [0xff; 6] {
        return Err("invalid peer MAC".into());
    }
    Ok(address)
}

fn display_address([f, e, d, c, b, a]: bt::Address) -> String {
    format!("{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{f:02X}")
}

enum Command {
    Status,
    Scan,
    Pair(bt::Address),
    Connect(bt::Address),
    Disconnect,
}

fn parse<'a>(args: &[&'a str]) -> Result<(&'a str, Command)> {
    let (path, command) = match args {
        ["--device", path, rest @ ..] if !path.is_empty() => (*path, rest),
        [path, rest @ ..] if path.starts_with('/') => (*path, rest),
        rest => (DEFAULT_DEVICE, rest),
    };
    let command = match command {
        ["status"] => Command::Status,
        ["scan"] => Command::Scan,
        ["pair", mac] => Command::Pair(parse_address(mac)?),
        ["connect", mac] => Command::Connect(parse_address(mac)?),
        ["disconnect"] => Command::Disconnect,
        _ => return Err(USAGE.into()),
    };
    Ok((path, command))
}

fn open_at(directory: usize, name: &CStr, flags: u32, mode: u32) -> io::Result<Handle> {
    let flags = flags | vfs::VFS_O_NOFOLLOW | vfs::VFS_O_CLOEXEC;
    // SAFETY: name is NUL-terminated and lives through the call. Success transfers one handle.
    let raw = unsafe {
        syscall4(
            Syscall::VfsOpenAt,
            directory,
            name.as_ptr() as usize,
            flags as usize,
            mode as usize,
        )
    } as isize;
    if raw < 0 {
        return Err(io::Error::from_raw_os_error((-raw) as i32));
    }
    // SAFETY: from_raw consumes our newly opened handle, including on failure.
    unsafe { Handle::from_raw(raw as i32) }.map_err(|_| io::Error::other("handle query failed"))
}

fn control(handle: &Handle, op: u32, record: &mut bt::Record) -> Result<()> {
    // SAFETY: only V1 Bluetooth commands from this module reach this call. Each borrows
    // this exact writable record synchronously, with no embedded or retained pointers.
    match unsafe { handle.control(op, record.as_mut_ptr() as usize) } {
        Ok(0) => Ok(()),
        Ok(code) => Err(format!("control {op:#010x} returned {code}")),
        Err(error) => Err(format!("control {op:#010x} failed ({error:?})")),
    }
}

fn flags(r: &bt::Record) -> u32 {
    u32::from_le_bytes(r[bt::FLAGS].try_into().unwrap())
}
fn generation(r: &bt::Record) -> u32 {
    u32::from_le_bytes(r[bt::GENERATION].try_into().unwrap())
}
fn last_error(r: &bt::Record) -> u32 {
    u32::from_le_bytes(r[bt::LAST_ERROR].try_into().unwrap())
}
fn count(r: &bt::Record) -> u16 {
    u16::from_le_bytes(r[bt::COUNT].try_into().unwrap())
}

fn status(handle: &Handle) -> Result<bt::Record> {
    let mut record = [0; bt::RECORD_SIZE];
    control(handle, bt::STATUS, &mut record)?;
    Ok(record)
}

fn print_status(r: &bt::Record) {
    let has = |bit| flags(r) & bit != 0;
    let peer = r[bt::PEER].try_into().unwrap();
    let peer = if peer == [0; 6] {
        "none".into()
    } else {
        display_address(peer)
    };
    println!(
        "adapter={} ready={} busy={} scanning={} found={} generation={}",
        display_address(r[bt::ADAPTER].try_into().unwrap()),
        has(bt::READY),
        has(bt::BUSY),
        has(bt::SCANNING),
        count(r),
        generation(r),
    );
    println!(
        "peer={peer} connected={} encrypted={} profile-ready={} key-readable={} last-error={:#x}",
        has(bt::CONNECTED),
        has(bt::ENCRYPTED),
        has(bt::PROFILE_READY),
        has(bt::KEY_READABLE),
        last_error(r),
    );
}

fn complete(r: &bt::Record, expected: u32) -> Result<bool> {
    if generation(r) != expected {
        return Err("another operation replaced this generation".into());
    }
    if flags(r) & (bt::BUSY | bt::SCANNING) != 0 {
        return Ok(false);
    }
    if last_error(r) != 0 {
        return Err(format!("controller last-error={:#x}", last_error(r)));
    }
    Ok(true)
}

fn start_and_wait(handle: &Handle, op: u32, request: &mut bt::Record) -> Result<bt::Record> {
    let submitted = control(handle, op, request);
    request[bt::KEY].zeroize();
    if let Err(error) = submitted {
        if let Ok(state) = status(handle) {
            print_status(&state);
        }
        return Err(error);
    }
    let expected = generation(request);
    let start = Instant::now();
    loop {
        let state = status(handle)?;
        if complete(&state, expected)? {
            return Ok(state);
        }
        if start.elapsed() >= Duration::from_secs(120) {
            return Err(format!("timed out waiting for generation {expected}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn scan_row(r: &bt::Record) -> Result<String> {
    let id = display_address(r[bt::PEER].try_into().unwrap());
    let bytes = r[bt::NAME]
        .get(..usize::from(r[bt::NAME_LEN]))
        .ok_or("invalid scan result name length")?;
    if bytes.is_empty() {
        return Ok(format!("{id}  (unknown name)"));
    }
    // Debug-quoted string escapes remote terminal controls, including embedded newlines.
    Ok(format!("{id}  {:?}", String::from_utf8_lossy(bytes)))
}

fn scan(handle: &Handle) -> Result<()> {
    let state = start_and_wait(handle, bt::SCAN_START, &mut [0; bt::RECORD_SIZE])?;
    let total = count(&state);
    for index in 0..total {
        let mut row = [0; bt::RECORD_SIZE];
        row[bt::GENERATION].copy_from_slice(&state[bt::GENERATION]);
        row[bt::INDEX].copy_from_slice(&index.to_le_bytes());
        control(handle, bt::SCAN_RESULT, &mut row)?;
        println!("{}", scan_row(&row)?);
    }
    println!("{total} device(s) found");
    Ok(())
}

mod bonds {
    use super::*;

    fn check_access(handle: &Handle, directory: bool) -> Result<()> {
        let access = scarlet_os::shield::inode_access(handle)
            .map_err(|_| "cannot verify bond storage ownership, mode and ACL")?;
        let forbidden = if directory { 0o022 } else { 0o7177 };
        if access.owner != 0 || access.has_acl != 0 || access.mode & forbidden != 0 {
            return Err("unsafe bond storage ownership, permissions or ACL".into());
        }
        // O_DIRECTORY already checked directories. A key, stage or lock must be
        // a regular file with no additional hard links.
        if !directory {
            let meta = handle
                .as_file()
                .map_err(|_| "expected a bond storage file")?
                .metadata()
                .map_err(|_| "cannot inspect bond storage file")?;
            if !meta.is_file() || meta.link_count != 1 {
                return Err("bond storage requires a singly linked regular file".into());
            }
        }
        Ok(())
    }

    fn sync(handle: &Handle) -> Result<()> {
        // SAFETY: the descriptor remains open through this scalar operation.
        let rc = unsafe { syscall1(Syscall::FileSync, handle.as_raw() as usize) } as isize;
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("bond storage sync failed ({rc})"))
        }
    }

    // Root-owned /etc is supplied by the base image. All children are flat and
    // generated exclusively from numeric BD_ADDRs. Each file holds type || 16 HCI octets.
    pub struct Store {
        dir: Handle,
        adapter: String,
        name: String,
    }

    impl Store {
        pub fn open(adapter: bt::Address, peer: bt::Address) -> Result<Self> {
            if adapter == [0; 6] {
                return Err("controller has no adapter address for bond storage".into());
            }
            let root = open_at(vfs::CURRENT_DIRECTORY, c"/", vfs::VFS_O_DIRECTORY, 0)
                .map_err(|e| format!("open bond storage root: {e}"))?;
            check_access(&root, true)?;
            let dir = open_at(root.as_raw() as usize, c"etc", vfs::VFS_O_DIRECTORY, 0)
                .map_err(|e| format!("open /etc for bond storage: {e}"))?;
            check_access(&dir, true)?;
            let adapter = display_address(adapter).replace(':', "");
            let peer = display_address(peer).replace(':', "");
            let name = format!("bluetooth-{adapter}-{peer}.bond");
            Ok(Self { dir, adapter, name })
        }

        fn file(&self, name: &str, flags: u32) -> Result<Option<Handle>> {
            let component = CString::new(name).unwrap();
            match open_at(self.dir.as_raw() as usize, &component, flags, 0o600) {
                Ok(file) => {
                    check_access(&file, false)?;
                    Ok(Some(file))
                }
                Err(e) if e.raw_os_error() == Some(vfs::ERRNO_ENOENT) => Ok(None),
                Err(e) => Err(format!("open /etc/{name}: {e}")),
            }
        }

        pub fn supply_key(&self, request: &mut bt::Record) -> Result<()> {
            let Some(file) = self.file(&self.name, vfs::VFS_O_RDONLY)? else {
                return Ok(());
            };
            let stream = file.as_stream().map_err(|_| "cannot read bond file")?;
            let mut bytes = Zeroizing::new([0; 18]);
            stream
                .read_exact(&mut bytes[..17])
                .map_err(|_| "invalid or unreadable bond file")?;
            if stream
                .read(&mut bytes[17..])
                .map_err(|_| "cannot finish reading bond file")?
                != 0
            {
                return Err("invalid bond file length".into());
            }
            request[bt::KEY_TYPE] = bytes[0];
            request[bt::KEY].copy_from_slice(&bytes[1..17]);
            request[bt::FLAGS].copy_from_slice(&bt::KEY_SUPPLIED.to_le_bytes());
            Ok(())
        }

        pub fn lock(&self, pairing: bool) -> Result<Option<Handle>> {
            let name = format!(".bluetooth-{}.lock", self.adapter);
            let flags = if pairing {
                vfs::VFS_O_CREAT | vfs::VFS_O_RDWR
            } else {
                vfs::VFS_O_RDONLY
            };
            let Some(lock) = self.file(&name, flags)? else {
                return if pairing {
                    Err("cannot create bond storage lock".into())
                } else {
                    Ok(None)
                };
            };
            // SAFETY: FileLock borrows this live handle. A nonblocking exclusive (pair)
            // or shared (connect) lock lasts through radio completion and bond installation.
            let operation = if pairing { 2 | 4 } else { 1 | 4 };
            let rc =
                unsafe { syscall2(Syscall::FileLock, lock.as_raw() as usize, operation) } as isize;
            if rc != 0 {
                return Err(format!("cannot lock adapter bond storage ({rc})"));
            }
            if pairing {
                // Reject unsafe files before changing the peer's bond.
                self.file(&self.name, vfs::VFS_O_RDONLY)?;
                self.file(&format!(".{}.new", self.name), vfs::VFS_O_WRONLY)?;
            }
            Ok(Some(lock))
        }

        pub fn save_key(&self, r: &bt::Record) -> Result<()> {
            // The stable adapter lock serializes this staging name. Reopen and
            // validate a stage left by an interrupted pair before modifying it.
            let stage = format!(".{}.new", self.name);
            let file = self
                .file(&stage, vfs::VFS_O_WRONLY | vfs::VFS_O_CREAT)?
                .ok_or("cannot create bond staging file")?;
            let old = CString::new(format!("/etc/{stage}")).unwrap();
            let new = CString::new(format!("/etc/{}", self.name)).unwrap();
            let install = (|| -> Result<()> {
                file.as_file()
                    .map_err(|_| "invalid staging file")?
                    .truncate(0)
                    .map_err(|_| "cannot truncate bond staging file")?;
                let stream = file
                    .as_stream()
                    .map_err(|_| "cannot open bond staging stream")?;
                stream
                    .write_all(std::slice::from_ref(&r[bt::KEY_TYPE]))
                    .and_then(|()| stream.write_all(&r[bt::KEY]))
                    .map_err(|_| "cannot write bond staging file")?;
                sync(&file)?;
                file.close().map_err(|_| "cannot close bond staging file")?;
                // SAFETY: both C strings live through the call and name children of
                // the validated root-owned /etc. Atomic replacement stays on one filesystem.
                let rc = unsafe {
                    syscall3(
                        Syscall::VfsRenameWithStatus,
                        old.as_ptr() as usize,
                        new.as_ptr() as usize,
                        0,
                    )
                } as isize;
                if rc != 0 {
                    return Err(format!("cannot install bond file ({rc})"));
                }
                Ok(())
            })();
            if install.is_err() {
                std::fs::remove_file(format!("/etc/{stage}"))
                    .map_err(|e| format!("bond installation and staging cleanup failed: {e}"))?;
            }
            install?;
            sync(&self.dir)
        }
    }
}

fn link(handle: &Handle, peer: bt::Address, pair: bool) -> Result<()> {
    let initial = status(handle)?;
    let adapter: bt::Address = initial[bt::ADAPTER].try_into().unwrap();
    let store = bonds::Store::open(adapter, peer)?;
    let mut request = Zeroizing::new([0; bt::RECORD_SIZE]);
    request[bt::PEER].copy_from_slice(&peer);
    let _lock = store.lock(pair)?;
    if !pair {
        store.supply_key(&mut request)?;
    }
    let op = if pair { bt::PAIR } else { bt::CONNECT };
    let state = start_and_wait(handle, op, &mut request)?;
    if state[bt::ADAPTER] != adapter
        || state[bt::PEER] != peer
        || flags(&state) & (bt::CONNECTED | bt::ENCRYPTED) != (bt::CONNECTED | bt::ENCRYPTED)
    {
        return Err("controller did not confirm an encrypted link to the requested peer".into());
    }
    if pair {
        if flags(&state) & bt::KEY_READABLE == 0 {
            return Err("paired, but the controller did not offer a bond key for storage".into());
        }
        request.zeroize();
        request[bt::GENERATION].copy_from_slice(&state[bt::GENERATION]);
        request[bt::PEER].copy_from_slice(&peer);
        control(handle, bt::READ_KEY, &mut request)
            .and_then(|()| store.save_key(&request))
            .map_err(|e| format!("paired, but could not save the bond: {e}"))?;
        println!("bond saved for {}", display_address(peer));
    }
    print_status(&state);
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    let (path, command) = parse(&words)?;
    let device = CString::new(path).map_err(|_| "invalid device path")?;
    let access = if matches!(command, Command::Status) {
        vfs::VFS_O_RDONLY
    } else {
        vfs::VFS_O_RDWR
    };
    let handle = open_at(vfs::CURRENT_DIRECTORY, &device, access, 0)
        .map_err(|e| format!("open {path}: {e}"))?;
    match command {
        Command::Status => print_status(&status(&handle)?),
        Command::Scan => scan(&handle)?,
        Command::Pair(peer) => link(&handle, peer, true)?,
        Command::Connect(peer) => link(&handle, peer, false)?,
        Command::Disconnect => {
            let state = start_and_wait(&handle, bt::DISCONNECT, &mut [0; bt::RECORD_SIZE])?;
            print_status(&state);
            if flags(&state) & bt::CONNECTED != 0 {
                return Err("controller still reports an active link".into());
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("btctl: {message}");
            ExitCode::FAILURE
        }
    }
}
