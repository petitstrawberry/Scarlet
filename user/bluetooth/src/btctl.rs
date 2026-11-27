#[cfg(not(any(target_os = "scarlet", test)))]
compile_error!("btctl requires a Scarlet Native target");

mod bonds;
mod broker;
mod reconnect;
mod reply;
use reply::Reply;

use scarlet_abi::{bluetooth as bt, fs as vfs};
use scarlet_bluetooth::{
    Address, ConnectedPeer, DiscoveredPeer, HidReconnectMode, Status,
    protocol::{self, Command},
};
use scarlet_os::Handle;
use scarlet_sys::{Syscall, syscall1, syscall2, syscall3, syscall4};
use std::ffi::{CStr, CString};
use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

const USAGE: &str = "usage: btctl [--device PATH | /dev/PATH] status|scan|devices|bonded|saved|pair MAC|connect MAC|disconnect [MAC]|forget MAC|unpair MAC";
const DEFAULT_DEVICE: &str = "/dev/bluetooth0";
type Result<T> = std::result::Result<T, String>;

fn parse<'a>(args: &[&'a str]) -> Result<(&'a str, Command)> {
    let (path, command) = match args {
        ["--device", path, rest @ ..] if !path.is_empty() => (*path, rest),
        [path, rest @ ..] if path.starts_with('/') => (*path, rest),
        rest => (DEFAULT_DEVICE, rest),
    };
    let command = match command {
        ["status"] => Command::Status,
        ["scan"] => Command::Scan,
        ["devices"] => Command::Devices,
        ["bonded"] => Command::Bonded,
        ["saved"] => Command::Saved,
        ["forget" | "unpair", mac] => Command::Forget(mac.parse::<Address>()?.0),
        ["pair", mac] => Command::Pair(mac.parse::<Address>()?.0),
        ["connect", mac] => Command::Connect(mac.parse::<Address>()?.0),
        ["disconnect"] => Command::Disconnect(None),
        ["disconnect", mac] => Command::Disconnect(Some(mac.parse::<Address>()?.0)),
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

fn record_peer(record: &bt::Record) -> Option<bt::Address> {
    let peer = record[bt::PEER].try_into().unwrap();
    (peer != [0; 6]).then_some(peer)
}

fn status(handle: &Handle, target: Option<bt::Address>) -> Result<bt::Record> {
    let mut record = [0; bt::RECORD_SIZE];
    if let Some(peer) = target {
        record[bt::PEER].copy_from_slice(&peer);
        control(handle, bt::PEER_STATUS, &mut record)?;
        if record_peer(&record) != target {
            return Err("controller returned status for a different peer".into());
        }
    } else {
        control(handle, bt::STATUS, &mut record)?;
    }
    Ok(record)
}

fn public_status(adapter: &bt::Record, peer: &bt::Record) -> Status {
    let adapter_flags = flags(adapter);
    let peer_flags = flags(peer);
    Status {
        adapter: Address(adapter[bt::ADAPTER].try_into().unwrap()),
        ready: adapter_flags & bt::READY != 0,
        busy: adapter_flags & bt::BUSY != 0,
        scanning: adapter_flags & bt::SCANNING != 0,
        found: count(adapter),
        generation: generation(adapter),
        peer: record_peer(peer).map(Address),
        connected: peer_flags & bt::CONNECTED != 0,
        encrypted: peer_flags & bt::ENCRYPTED != 0,
        profile_ready: peer_flags & bt::PROFILE_READY != 0,
        key_readable: peer_flags & bt::KEY_READABLE != 0,
        last_error: last_error(peer),
        hid_reconnect_mode: None,
    }
}

fn hid_mode(peer: &bt::Record) -> Option<HidReconnectMode> {
    match peer[bt::HID_RECONNECT_MODE] {
        0 if flags(peer) & bt::PROFILE_READY != 0 => Some(HidReconnectMode::None),
        bt::HID_RECONNECT_DEVICE => Some(HidReconnectMode::Device),
        bt::HID_RECONNECT_HOST => Some(HidReconnectMode::Host),
        bits if bits == bt::HID_RECONNECT_HOST | bt::HID_RECONNECT_DEVICE => {
            Some(HidReconnectMode::Any)
        }
        _ => None,
    }
}

fn complete(r: &bt::Record, expected: u32) -> Result<bool> {
    if generation(r) != expected {
        return Err("another operation replaced this generation".into());
    }
    if flags(r) & (bt::BUSY | bt::SCANNING) != 0 {
        return Ok(false);
    }
    let error = last_error(r);
    if error != 0 {
        let hint = match error {
            0x04 => " (Page Timeout). Wake the device, bring it within range and retry.",
            0x05 => {
                " (Authentication Failure). If it persists, forget the bond on both devices and pair again."
            }
            0x06 => {
                " (PIN or Key Missing). Put the peer in pairing mode and pair again to save a new key."
            }
            _ => "",
        };
        return Err(format!("controller last-error={error:#04x}{hint}"));
    }
    Ok(true)
}

fn start_and_wait(handle: &Handle, op: u32, request: &mut bt::Record) -> Result<bt::Record> {
    let requested_peer = record_peer(request);
    let submitted = control(handle, op, request);
    request[bt::KEY].zeroize();
    if let Err(error) = submitted {
        if op == bt::DISCONNECT {
            let mut links = [0; bt::RECORD_SIZE];
            if control(handle, bt::LINKS, &mut links).is_ok() && count(&links) > 1 {
                return Err(
                    "multiple connected peers; use btctl devices and btctl disconnect MAC".into(),
                );
            }
        }
        if let Ok(state) = status(handle, None) {
            return Err(format!(
                "{error}\n{}",
                reply::status_text(&public_status(&state, &state))
            ));
        }
        return Err(error);
    }
    // Older single-link controllers return only GENERATION. New link operations
    // also return the selected PEER, including an atomic bare DISCONNECT.
    let target = record_peer(request);
    if target.is_some() && requested_peer.is_some() && target != requested_peer {
        return Err("controller accepted a different peer".into());
    }
    if op == bt::DISCONNECT_PEER && target.is_none() {
        return Err("controller did not confirm the disconnect target".into());
    }
    let expected = generation(request);
    let start = Instant::now();
    loop {
        let state = status(handle, target)?;
        if complete(&state, expected)? {
            return Ok(state);
        }
        if start.elapsed() >= Duration::from_secs(120) {
            return Err(format!("timed out waiting for generation {expected}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn peer_name(r: &bt::Record) -> Result<Option<String>> {
    let bytes = r[bt::NAME]
        .get(..usize::from(r[bt::NAME_LEN]))
        .ok_or("invalid peer name length")?;
    Ok((!bytes.is_empty()).then(|| String::from_utf8_lossy(bytes).into_owned()))
}

fn indexed_record(snapshot: &bt::Record, index: u16) -> bt::Record {
    let mut row = [0; bt::RECORD_SIZE];
    row[bt::GENERATION].copy_from_slice(&snapshot[bt::GENERATION]);
    row[bt::INDEX].copy_from_slice(&index.to_le_bytes());
    row
}

fn scan_results(handle: &Handle, state: &bt::Record) -> Result<Vec<DiscoveredPeer>> {
    let mut listing = Vec::new();
    let total = count(state);
    for index in 0..total {
        let mut row = indexed_record(state, index);
        control(handle, bt::SCAN_RESULT, &mut row)?;
        let address = Address(record_peer(&row).ok_or("invalid discovery address")?);
        listing.push(DiscoveredPeer {
            address,
            name: peer_name(&row)?,
        });
    }
    Ok(listing)
}

// A remote disconnect can invalidate an index while we enumerate. Discard the
// partial listing and retry only if the controller actually changed snapshots.
fn devices(mut ctl: impl FnMut(u32, &mut bt::Record) -> Result<()>) -> Result<Vec<ConnectedPeer>> {
    let mut snapshot = [0; bt::RECORD_SIZE];
    ctl(bt::LINKS, &mut snapshot)?;
    device_rows(&mut snapshot, ctl)
}

fn device_rows(
    snapshot: &mut bt::Record,
    mut ctl: impl FnMut(u32, &mut bt::Record) -> Result<()>,
) -> Result<Vec<ConnectedPeer>> {
    for _ in 0..3 {
        let token = generation(snapshot);
        let total = count(snapshot);
        let mut listing = Vec::new();
        for index in 0..total {
            let mut row = indexed_record(snapshot, index);
            if let Err(error) = ctl(bt::LINK_RESULT, &mut row) {
                snapshot.fill(0);
                ctl(bt::LINKS, snapshot)?;
                if generation(snapshot) != token {
                    break;
                }
                return Err(error);
            }
            if generation(&row) != token
                || record_peer(&row).is_none()
                || flags(&row) & bt::CONNECTED == 0
            {
                return Err("invalid connected-peer snapshot row".into());
            }
            listing.push(ConnectedPeer {
                address: Address(row[bt::PEER].try_into().unwrap()),
                name: peer_name(&row)?,
                encrypted: flags(&row) & bt::ENCRYPTED != 0,
                profile_ready: flags(&row) & bt::PROFILE_READY != 0,
                key_readable: flags(&row) & bt::KEY_READABLE != 0,
                last_error: last_error(&row),
                hid_reconnect_mode: hid_mode(&row),
            });
        }
        if generation(snapshot) == token {
            return Ok(listing);
        }
    }
    Err("connected peers kept changing; retry btctl devices".into())
}

fn link(handle: &Handle, peer: bt::Address, pair: bool) -> Result<Status> {
    let initial = status(handle, None)?;
    let adapter: bt::Address = initial[bt::ADAPTER].try_into().unwrap();
    let store = bonds::Store::locked(adapter)?;
    let mut request = Zeroizing::new([0; bt::RECORD_SIZE]);
    request[bt::PEER].copy_from_slice(&peer);
    if pair {
        store.check_pair(Address(peer))?;
    } else {
        store.load_key(Address(peer), &mut request)?;
        request[bt::FLAGS].copy_from_slice(&bt::KEY_SUPPLIED.to_le_bytes());
    }
    let op = if pair { bt::PAIR } else { bt::CONNECT };
    let state = start_and_wait(handle, op, &mut request)?;
    if state[bt::ADAPTER] != adapter
        || state[bt::PEER] != peer
        || flags(&state) & (bt::CONNECTED | bt::ENCRYPTED) != (bt::CONNECTED | bt::ENCRYPTED)
    {
        return Err("controller did not confirm an encrypted link to the requested peer".into());
    }
    let target = record_peer(&request);
    if pair {
        if flags(&state) & bt::KEY_READABLE == 0 {
            return Err("paired, but the controller did not offer a bond key for storage".into());
        }
        request.zeroize();
        request[bt::GENERATION].copy_from_slice(&state[bt::GENERATION]);
        request[bt::PEER].copy_from_slice(&peer);
        control(handle, bt::READ_KEY, &mut request)
            .and_then(|()| store.save_key(Address(peer), &request))
            .map_err(|e| format!("paired, but could not save the bond: {e}"))?;
        request.zeroize();
        store
            .register(handle, Address(peer))
            .map_err(|e| format!("bond saved, but automatic reconnect registration failed: {e}"))?;
        store
            .remember(Address(peer), peer_name(&state)?.as_deref())
            .map_err(|e| format!("bond saved, but could not save the device name: {e}"))?;
        return status(handle, target)
            .and_then(|state| completion_status(handle, target, &state))
            .map_err(|e| {
                format!(
                    "bond saved for {}, but status refresh failed: {e}",
                    Address(peer)
                )
            });
    }
    store.remember(Address(peer), peer_name(&state)?.as_deref())?;
    completion_status(handle, target, &state)
}

fn completion_status(
    handle: &Handle,
    target: Option<bt::Address>,
    state: &bt::Record,
) -> Result<Status> {
    if target.is_some() {
        Ok(Status {
            hid_reconnect_mode: hid_mode(state),
            ..public_status(&status(handle, None)?, state)
        })
    } else {
        Ok(public_status(state, state))
    }
}

fn disconnect(handle: &Handle, target: Option<bt::Address>) -> Result<Status> {
    let mut request = [0; bt::RECORD_SIZE];
    let op = if let Some(peer) = target {
        reconnect::cancel(
            status(handle, None)?[bt::ADAPTER].try_into().unwrap(),
            Address(peer),
        );
        request[bt::PEER].copy_from_slice(&peer);
        bt::DISCONNECT_PEER
    } else {
        bt::DISCONNECT
    };
    let state = start_and_wait(handle, op, &mut request)?;
    if let Some(peer) = record_peer(&request) {
        reconnect::cancel(state[bt::ADAPTER].try_into().unwrap(), Address(peer));
    }
    if flags(&state) & bt::CONNECTED != 0 {
        return Err("controller still reports the peer connected".into());
    }
    if target.is_none() && record_peer(&request).is_some() {
        // Preserve the legacy adapter summary after implicitly detaching its sole peer.
        let adapter = status(handle, None)?;
        return Ok(public_status(&adapter, &adapter));
    }
    completion_status(handle, target, &state)
}

fn saved(handle: &Handle) -> Result<Vec<DiscoveredPeer>> {
    let state = status(handle, None)?;
    let adapter = state[bt::ADAPTER].try_into().unwrap();
    let store = bonds::Store::locked(adapter)?;
    let mut listing = Vec::new();
    let mut discovery = None;
    for address in bonds::list(adapter)? {
        // PEER_STATUS includes names resolved after pairing, even after disconnect.
        // An older bond may have no controller history at all.
        let recent = status(handle, Some(address.0))
            .ok()
            .map(|row| peer_name(&row))
            .transpose()?
            .flatten();
        let mut name = store.name(address)?;
        let mut observed = recent;
        if name.is_none() && observed.is_none() && flags(&state) & bt::SCANNING == 0 {
            if discovery.is_none() {
                discovery = Some(scan_results(handle, &state)?);
            }
            observed = discovery
                .as_ref()
                .unwrap()
                .iter()
                .find(|row| row.address == address)
                .and_then(|row| row.name.clone());
        }
        if observed.is_some() && observed != name {
            store.remember(address, observed.as_deref())?;
            name = observed;
        }
        listing.push(DiscoveredPeer { address, name });
    }
    Ok(listing)
}

fn forget(handle: &Handle, peer: bt::Address) -> Result<()> {
    let state = status(handle, None)?;
    let adapter: bt::Address = state[bt::ADAPTER].try_into().unwrap();
    reconnect::cancel(adapter, Address(peer));
    let store = bonds::Store::locked(adapter)?;
    let live = devices(|op, row| control(handle, op, row))?;
    if live.iter().any(|row| row.address == Address(peer)) {
        disconnect(handle, Some(peer))?;
    }
    let mut request = [0; bt::RECORD_SIZE];
    request[bt::PEER].copy_from_slice(&peer);
    let done = start_and_wait(handle, bt::EVICT_KEY, &mut request)?;
    if record_peer(&request) != Some(peer)
        || done[bt::ADAPTER] != adapter
        || flags(&done) & (bt::CONNECTED | bt::ENCRYPTED | bt::KEY_READABLE) != 0
    {
        return Err("controller did not confirm key eviction for the requested peer".into());
    }
    store.forget(Address(peer))
}

fn direct(path: &str, command: Command) -> Result<Reply> {
    let device = CString::new(path).map_err(|_| "invalid device path")?;
    let access = if matches!(
        command,
        Command::Status | Command::Devices | Command::Bonded | Command::Saved
    ) {
        vfs::VFS_O_RDONLY
    } else {
        vfs::VFS_O_RDWR
    };
    let handle = open_at(vfs::CURRENT_DIRECTORY, &device, access, 0)
        .map_err(|e| format!("open {path}: {e}"))?;
    match command {
        Command::Status => {
            let state = status(&handle, None)?;
            let mut summary = public_status(&state, &state);
            if let Some(peer) = record_peer(&state) {
                summary.hid_reconnect_mode = status(&handle, Some(peer))
                    .ok()
                    .filter(|row| flags(row) & bt::CONNECTED != 0)
                    .and_then(|row| hid_mode(&row));
            }
            Ok(Reply::Status(summary))
        }
        Command::Devices => {
            let rows = devices(|op, record| control(&handle, op, record))?;
            if rows.iter().any(|row| row.name.is_some()) {
                let state = status(&handle, None)?;
                let store = bonds::Store::locked(state[bt::ADAPTER].try_into().unwrap())?;
                store.remember_connected(&rows)?;
            }
            Ok(Reply::Devices(rows))
        }
        Command::Bonded => {
            let state = status(&handle, None)?;
            bonds::list(state[bt::ADAPTER].try_into().unwrap()).map(Reply::Bonded)
        }
        Command::Saved => saved(&handle).map(Reply::Saved),
        Command::Forget(peer) => forget(&handle, peer).map(Reply::Forgotten),
        Command::Scan => {
            let state = start_and_wait(&handle, bt::SCAN_START, &mut [0; bt::RECORD_SIZE])?;
            let rows = scan_results(&handle, &state)?;
            if rows.iter().any(|row| row.name.is_some()) {
                let store = bonds::Store::locked(state[bt::ADAPTER].try_into().unwrap())?;
                for row in &rows {
                    store.remember(row.address, row.name.as_deref())?;
                }
            }
            Ok(Reply::Scan(rows))
        }
        Command::Pair(peer) => link(&handle, peer, true).map(Reply::Status),
        Command::Connect(peer) => link(&handle, peer, false).map(Reply::Status),
        Command::Disconnect(target) => disconnect(&handle, target).map(Reply::Status),
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    if words == ["--serve"] {
        return broker::serve();
    }
    let (path, command) = parse(&words)?;
    let response = if let Some(adapter) = broker::adapter_number(path) {
        broker::invoke(adapter, command, || {
            direct(path, command).map(|reply| reply.text(command))
        })?
    } else {
        direct(path, command)?.text(command)
    };
    println!("{response}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_errors_retain_controller_codes_and_specific_recovery() {
        let mut record = [0; bt::RECORD_SIZE];
        for (code, hint) in [
            (0x04u32, "Wake the device"),
            (0x05, "Authentication Failure"),
            (0x06, "pair again"),
            (0x123, ""),
        ] {
            record[bt::LAST_ERROR].copy_from_slice(&code.to_le_bytes());
            let error = complete(&record, 0).unwrap_err();
            assert!(error.contains(&format!("{code:#04x}")) && error.contains(hint));
        }
    }

    #[test]
    fn devices_discards_partial_snapshots_after_a_remote_disconnect() {
        let mut round = 0u32;
        let listing = devices(|op, record| {
            match op {
                bt::LINKS => {
                    round += 1;
                    let count: u16 = if round == 1 { 2 } else { 1 };
                    record[bt::COUNT].copy_from_slice(&count.to_le_bytes());
                    record[bt::GENERATION].copy_from_slice(&round.to_le_bytes());
                }
                bt::LINK_RESULT => {
                    let index = u16::from_le_bytes(record[bt::INDEX].try_into().unwrap());
                    assert_eq!(generation(record), round);
                    if round == 1 && index == 1 {
                        return Err("stale snapshot".into());
                    }
                    let octet = if round == 1 { 0x10 } else { 0x20 };
                    record[bt::PEER].copy_from_slice(&[octet, 0, 0, 0, 0, 2]);
                    record[bt::FLAGS]
                        .copy_from_slice(&(bt::CONNECTED | bt::ENCRYPTED).to_le_bytes());
                    record[bt::NAME_LEN] = 2;
                    record[bt::NAME.start..bt::NAME.start + 2].copy_from_slice(b"R\n");
                }
                _ => panic!("unexpected opcode"),
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(round, 2);
        assert_eq!(
            Reply::Devices(listing).text(Command::Devices),
            "peer=02:00:00:00:00:20 connected=true encrypted=true profile-ready=false key-readable=false last-error=0x0 name=\"R\\n\"\n1 device(s) connected"
        );
    }

    #[test]
    fn devices_propagates_permission_failures_without_returning_an_empty_list() {
        let denied = "management denied";
        let mut calls = Vec::new();
        let result = devices(|op, record| {
            calls.push(op);
            if op == bt::LINKS {
                record[bt::COUNT].copy_from_slice(&1u16.to_le_bytes());
                return Ok(());
            }
            Err(denied.into())
        });
        assert_eq!(result, Err(denied.into()));
        assert_eq!(calls, [bt::LINKS, bt::LINK_RESULT, bt::LINKS]);
        assert_eq!(devices(|_, _| Err(denied.into())), Err(denied.into()));
    }
}
