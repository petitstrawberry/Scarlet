//! One authenticated request per stream. The root service owns all controller
//! and bond operations. Check the protected login database for non-root callers.
use super::{
    Command, Result, bonds, bt, control, count, device_rows, direct, flags, generation, open_at,
    reconnect, status, vfs,
};
use scarlet_bluetooth::protocol::{self, ENDPOINT, MAX_REPLY};
use scarlet_os::socket::{Socket, SocketError};
use std::{collections::BTreeMap, ffi::CString, thread};

pub(super) fn adapter_number(path: &str) -> Option<u32> {
    let suffix = path.strip_prefix("/dev/bluetooth")?;
    let number: u32 = suffix.parse().ok()?;
    (number.to_string() == suffix).then_some(number)
}

#[derive(Default)]
struct AdapterCache {
    restored: Option<(bt::Address, u64)>,
    links: Option<u32>,
}

fn restore_adapter(path: &str, cache: &mut AdapterCache) -> Result<()> {
    let path = CString::new(path).map_err(|_| "invalid adapter path")?;
    let handle = open_at(vfs::CURRENT_DIRECTORY, &path, vfs::VFS_O_RDWR, 0)
        .map_err(|e| format!("open controller: {e}"))?;
    let state = status(&handle, None)?;
    if flags(&state) & bt::READY == 0 {
        if let Some((adapter, _)) = cache.restored {
            reconnect::reset(adapter);
        }
        *cache = AdapterCache::default();
        return Ok(());
    }
    let adapter = state[bt::ADAPTER].try_into().unwrap();
    let epoch = u64::from_le_bytes(state[bt::ADAPTER_GENERATION].try_into().unwrap());
    let identity = (adapter, epoch);
    let restore = cache.restored != Some(identity);
    if restore && let Some((previous, _)) = cache.restored {
        reconnect::reset(previous);
    }
    let mut snapshot = [0; bt::RECORD_SIZE];
    control(&handle, bt::LINKS, &mut snapshot)?;
    let changed = restore || cache.links != Some(generation(&snapshot));
    if restore || (changed && count(&snapshot) != 0) {
        let Some(store) = bonds::Store::try_lock(adapter)? else {
            return Ok(());
        };
        if restore {
            // Enumerate, load and register under the forget/pair lock. An error
            // leaves this initialization pending for the next pass.
            store.register_all(&handle)?;
            cache.restored = Some(identity);
            cache.links = None;
        }
        if changed {
            let rows = device_rows(&mut snapshot, |op, row| control(&handle, op, row))?;
            reconnect::observe(&handle, adapter, &rows);
            store.remember_connected(&rows)?;
        }
    } else if changed {
        reconnect::observe(&handle, adapter, &[]);
    }
    cache.links = Some(generation(&snapshot));
    if flags(&state) & bt::BUSY == 0 {
        reconnect::poll(&handle, adapter)?;
    }
    Ok(())
}

fn restore_adapters(cache: &mut BTreeMap<u32, AdapterCache>) -> Result<()> {
    let mut present = BTreeMap::new();
    let mut first_error = None;
    for entry in std::fs::read_dir("/dev").map_err(|e| format!("list controllers: {e}"))? {
        let path = entry
            .map_err(|e| format!("read controller entry: {e}"))?
            .path();
        let Some(path) = path.to_str() else { continue };
        let Some(number) = adapter_number(path) else {
            continue;
        };
        let mut adapter = cache.remove(&number).unwrap_or_default();
        if let Err(e) = restore_adapter(path, &mut adapter) {
            first_error.get_or_insert_with(|| format!("{path}: {e}"));
        }
        present.insert(number, adapter);
    }
    for absent in cache.values() {
        if let Some((adapter, _)) = absent.restored {
            reconnect::reset(adapter);
        }
    }
    *cache = present;
    first_error.map_or(Ok(()), Err)
}

fn restore_worker() {
    let mut cache = BTreeMap::new();
    let mut last_error = None;
    loop {
        let error = restore_adapters(&mut cache).err();
        if error != last_error
            && let Some(message) = &error
        {
            eprintln!("Bluetooth bond registration: {message}");
        }
        last_error = error;
        thread::sleep(reconnect::POLL);
    }
}

fn allowed(uid: u32, flags: u32, login_enabled: impl FnOnce(u32) -> bool) -> bool {
    // Whitelist exact credentials so unknown restrictions also fail closed.
    if !matches!(flags, 0 | scarlet_abi::LOCAL_PEER_MANAGE_DEVICES) {
        return false;
    }
    uid == 0 || (uid != u32::MAX && login_enabled(uid))
}

fn handle(socket: Socket) -> Result<()> {
    protocol::timeouts(&socket, 5000)?;
    let (uid, flags) = protocol::credentials(&socket)?;
    #[cfg(target_os = "scarlet")]
    let authorized = allowed(uid, flags, scarlet_accounts::login_uid_enabled);
    #[cfg(not(target_os = "scarlet"))]
    let authorized = allowed(uid, flags, |_| false);
    let stream = protocol::Stream::new(&socket)?;
    stream.write_all(&[u8::from(authorized)])?;
    if !authorized {
        return Ok(());
    }
    let mut request = [0; 11];
    stream.read_exact(&mut request)?;
    let result = protocol::decode(request).and_then(|(adapter, cmd, typed)| {
        let reply = direct(&format!("/dev/bluetooth{adapter}"), cmd)?;
        let bytes = if typed {
            serde_json::to_vec(&reply).map_err(|e| format!("encode Bluetooth reply: {e}"))?
        } else {
            reply.text(cmd).into_bytes()
        };
        if bytes.len() > MAX_REPLY {
            return Err("Bluetooth listing exceeds the service response limit".into());
        }
        Ok(bytes)
    });
    let (code, bytes) = match result {
        Ok(bytes) => (0, bytes),
        Err(error) => (1, error.into_bytes()),
    };
    let mut header = [code; 5];
    header[1..].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    stream.write_all(&header)?;
    stream.write_all(&bytes)
}

pub(super) fn serve() -> Result<()> {
    let listener = Socket::new().map_err(|e| format!("create Bluetooth listener ({e:?})"))?;
    listener
        .bind_abstract(ENDPOINT)
        .and_then(|()| listener.listen(8))
        .map_err(|e| format!("listen for Bluetooth clients ({e:?})"))?;
    thread::Builder::new()
        .name("bluetooth-bonds".into())
        .spawn(restore_worker)
        .map_err(|e| format!("start Bluetooth bond registration: {e}"))?;
    loop {
        match listener.accept() {
            Ok(socket) => {
                if let Err(error) = handle(socket) {
                    eprintln!("Bluetooth service: {error}");
                }
            }
            Err(SocketError::Interrupted) => continue,
            Err(e) => return Err(format!("accept Bluetooth client ({e:?})")),
        }
    }
}

pub(super) fn invoke(
    adapter: u32,
    cmd: Command,
    recovery: impl FnOnce() -> Result<String>,
) -> Result<String> {
    match protocol::open() {
        Ok(socket) => String::from_utf8(protocol::exchange(&socket, adapter, cmd, false)?)
            .map_err(|_| "invalid Bluetooth service text".into()),
        Err(e) => {
            recovery().map_err(|error| format!("Bluetooth service unavailable ({e:?}); {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use scarlet_abi::{LOCAL_PEER_CONFINED as CONFINED, LOCAL_PEER_MANAGE_DEVICES as MANAGE};
    use scarlet_bluetooth::protocol::decode;

    #[test]
    fn restore_accepts_canonical_numbered_adapter_paths() {
        assert_eq!(adapter_number("/dev/bluetooth0"), Some(0));
        assert_eq!(adapter_number("/dev/bluetooth17"), Some(17));
        for path in [
            "/dev/bluetooth",
            "/dev/bluetooth00",
            "/dev/bluetooth+1",
            "/dev/bluetooth4294967296",
            "/dev/bluetooth3/other",
            "/tmp/bluetooth0",
        ] {
            assert_eq!(adapter_number(path), None);
        }
    }

    #[test]
    fn public_commands_keep_wire_verbs_and_mac_order() {
        let mac = "A1:B2:C3:D4:E5:F6";
        let peer = [0xf6, 0xe5, 0xd4, 0xc3, 0xb2, 0xa1];
        for (args, verb, address) in [
            (&["status"][..], 1, [0; 6]),
            (&["scan"], 2, [0; 6]),
            (&["pair", mac], 3, peer),
            (&["connect", mac], 4, peer),
            (&["disconnect"], 5, [0; 6]),
            (&["devices"], 6, [0; 6]),
            (&["bonded"], 8, [0; 6]),
            (&["saved"], 9, [0; 6]),
            (&["forget", mac], 10, peer),
            (&["unpair", mac], 10, peer),
            (&["disconnect", mac], 7, peer),
        ] {
            let (_, command) = parse(args).unwrap();
            let adapter = 0x11223344;
            let mut frame = protocol::encode(adapter, command);
            assert_eq!(frame[..5], [verb, 0x44, 0x33, 0x22, 0x11]);
            assert_eq!(frame[5..], address);
            assert_eq!(decode(frame), Ok((adapter, command, false)));
            frame[0] |= 0x80;
            assert_eq!(decode(frame), Ok((adapter, command, true)));
        }
    }

    #[test]
    fn disconnect_cannot_downgrade_an_invalid_target_to_the_implicit_form() {
        let mut frame = protocol::encode(0, Command::Disconnect(None));
        frame[5..].fill(1);
        assert!(decode(frame).is_err());
        for address in [[0; 6], [255; 6]] {
            assert!(decode(protocol::encode(0, Command::Disconnect(Some(address)))).is_err());
            assert!(decode(protocol::encode(0, Command::Forget(address))).is_err());
        }
        assert!(parse(&["forget"]).is_err());
        assert!(parse(&["unpair"]).is_err());
        assert!(parse(&["saved", "extra"]).is_err());
        assert!(parse(&["disconnect", "00:00:00:00:00:00"]).is_err());
        assert!(parse(&["disconnect", "FF:FF:FF:FF:FF:FF"]).is_err());
        assert!(parse(&["devices", "extra"]).is_err());
    }

    #[test]
    fn only_unconfined_root_or_enabled_logins_can_call() {
        for flags in [0, MANAGE] {
            assert!(allowed(0, flags, |_| false));
            assert!(allowed(1001, flags, |uid| uid == 1001));
            assert!(!allowed(1002, flags, |_| false));
            assert!(!allowed(u32::MAX, flags, |_| true));
        }
        for uid in [0, 1001] {
            for flags in [CONFINED, MANAGE | CONFINED, 4, u32::MAX] {
                assert!(!allowed(uid, flags, |_| panic!(
                    "checked a confined account"
                )));
            }
        }
    }
}
