//! One authenticated request per stream. The root service owns all controller
//! and bond operations. Check the protected login database for non-root callers.
use super::{Command, Result, direct};
use scarlet_bluetooth::protocol::{self, ENDPOINT, MAX_REPLY};
use scarlet_os::socket::{Socket, SocketError};
pub(super) fn adapter_number(path: &str) -> Option<u32> {
    let suffix = path.strip_prefix("/dev/bluetooth")?;
    let number: u32 = suffix.parse().ok()?;
    (number.to_string() == suffix).then_some(number)
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
        }
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
