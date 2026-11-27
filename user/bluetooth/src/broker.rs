//! One request per connected stream. Both ends authenticate the kernel's immutable
//! peer snapshot before exchanging commands. Only the service opens a numbered
//! adapter and translates public commands to device controls.
use super::{Command, Result, direct};
use scarlet_os::socket::{Socket, SocketError};
use std::io::{Read, Write};

const ENDPOINT: &str = "scarlet.bluetooth.v1";
pub(super) const MAX_REPLY: usize = 128 * 1024;
const DENIED: &str = "Bluetooth management requires an unconfined root login";

pub(super) fn adapter_number(path: &str) -> Option<u32> {
    let suffix = path.strip_prefix("/dev/bluetooth")?;
    let number: u32 = suffix.parse().ok()?;
    (number.to_string() == suffix).then_some(number)
}

#[derive(Clone, Copy)]
enum Peer {
    Caller,
    Service,
}

impl Peer {
    fn accepts(self, principal: u32, flags: u32) -> bool {
        if principal != 0 {
            return false;
        }
        // Whitelist known credentials. A new restriction flag must fail closed.
        match self {
            Self::Caller => matches!(flags, 0 | scarlet_abi::LOCAL_PEER_MANAGE_DEVICES),
            Self::Service => flags == scarlet_abi::LOCAL_PEER_MANAGE_DEVICES,
        }
    }
}

fn trusted(socket: &Socket, peer: Peer) -> Result<bool> {
    let mut output = [0xff; 8];
    // SAFETY: the local socket control synchronously fills an eight-byte buffer.
    unsafe {
        socket.as_handle().control(
            scarlet_abi::SCTL_LOCAL_PEER_CREDENTIALS,
            output.as_mut_ptr() as usize,
        )
    }
    .map_err(|e| format!("cannot authenticate Bluetooth IPC peer ({e:?})"))?;
    let principal = u32::from_le_bytes(output[..4].try_into().unwrap());
    let flags = u32::from_le_bytes(output[4..].try_into().unwrap());
    Ok(peer.accepts(principal, flags))
}

fn timeouts(socket: &Socket, read_ms: usize) -> Result<()> {
    for (op, timeout) in [
        (scarlet_abi::SCTL_SOCKET_SET_READ_TIMEOUT_MS, read_ms),
        (scarlet_abi::SCTL_SOCKET_SET_WRITE_TIMEOUT_MS, 5000),
    ] {
        // SAFETY: both timeout controls take scalar milliseconds.
        unsafe { socket.as_handle().control(op, timeout) }
            .map_err(|e| format!("cannot set Bluetooth IPC timeout ({e:?})"))?;
    }
    Ok(())
}

// V1 request: verb, adapter number (LE u32), peer (six HCI-order bytes).
fn encode(adapter: u32, command: Command) -> [u8; 11] {
    let (verb, peer) = match command {
        Command::Status => (1, [0; 6]),
        Command::Scan => (2, [0; 6]),
        Command::Pair(peer) => (3, peer),
        Command::Connect(peer) => (4, peer),
        Command::Disconnect(None) => (5, [0; 6]),
        Command::Devices => (6, [0; 6]),
        Command::Disconnect(Some(peer)) => (7, peer),
    };
    let mut request = [0; 11];
    request[0] = verb;
    request[1..5].copy_from_slice(&adapter.to_le_bytes());
    request[5..].copy_from_slice(&peer);
    request
}

fn decode(request: [u8; 11]) -> Result<(u32, Command)> {
    let peer = request[5..].try_into().unwrap();
    let command = match request[0] {
        1 if peer == [0; 6] => Command::Status,
        2 if peer == [0; 6] => Command::Scan,
        3 if peer != [0; 6] && peer != [255; 6] => Command::Pair(peer),
        4 if peer != [0; 6] && peer != [255; 6] => Command::Connect(peer),
        5 if peer == [0; 6] => Command::Disconnect(None),
        6 if peer == [0; 6] => Command::Devices,
        7 if peer != [0; 6] && peer != [255; 6] => Command::Disconnect(Some(peer)),
        _ => return Err("invalid Bluetooth service request".into()),
    };
    let adapter = u32::from_le_bytes(request[1..5].try_into().unwrap());
    Ok((adapter, command))
}

fn ipc_error(e: std::io::Error) -> String {
    format!("Bluetooth service I/O failed ({e})")
}

fn handle(mut socket: Socket) -> Result<()> {
    timeouts(&socket, 5000)?;
    let allowed = trusted(&socket, Peer::Caller)?;
    socket.write_all(&[u8::from(allowed)]).map_err(ipc_error)?;
    if !allowed {
        return Ok(());
    }
    let mut request = [0; 11];
    socket.read_exact(&mut request).map_err(ipc_error)?;
    let response = decode(request)
        .and_then(|(adapter, command)| direct(&format!("/dev/bluetooth{adapter}"), command));
    let (status, message) = match response {
        Ok(text) => (0, text),
        Err(text) => (1, text),
    };
    // Replies contain only rendered status, scan rows or diagnostics.
    let mut header = [status; 5];
    header[1..].copy_from_slice(&(message.len() as u32).to_le_bytes());
    socket
        .write_all(&header)
        .and_then(|()| socket.write_all(message.as_bytes()))
        .map_err(ipc_error)
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
    command: Command,
    direct_request: impl FnOnce() -> Result<String>,
) -> Result<String> {
    let connection = Socket::new().and_then(|socket| {
        socket.connect_abstract(ENDPOINT)?;
        Ok(socket)
    });
    let mut socket = match connection {
        Ok(socket) => socket,
        // Recovery environments may already hold physical-seat management authority.
        // No command has been sent, so this cannot repeat a mutation.
        Err(e) => {
            return direct_request()
                .map_err(|error| format!("Bluetooth service unavailable ({e:?}); {error}"));
        }
    };
    if !trusted(&socket, Peer::Service)? {
        return Err("Bluetooth endpoint does not belong to a privileged system service".into());
    }
    timeouts(&socket, 130_000)?;
    let mut allowed = [0];
    socket.read_exact(&mut allowed).map_err(ipc_error)?;
    if allowed != [1] {
        return Err(DENIED.into());
    }
    socket
        .write_all(&encode(adapter, command))
        .map_err(ipc_error)?;
    let mut header = [0; 5];
    socket.read_exact(&mut header).map_err(ipc_error)?;
    let length = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
    if header[0] > 1 || length > MAX_REPLY {
        return Err("invalid Bluetooth service reply".into());
    }
    let mut message = vec![0; length];
    socket.read_exact(&mut message).map_err(ipc_error)?;
    let text = String::from_utf8(message).map_err(|_| "invalid Bluetooth service text")?;
    if header[0] == 0 { Ok(text) } else { Err(text) }
}

#[cfg(test)]
mod tests {
    use super::{Command, Peer, decode, encode};
    use crate::parse;
    use scarlet_abi::{LOCAL_PEER_CONFINED as CONFINED, LOCAL_PEER_MANAGE_DEVICES as MANAGE};

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
            (&["disconnect", mac], 7, peer),
        ] {
            let (_, command) = parse(args).unwrap();
            let adapter = 0x11223344;
            let frame = encode(adapter, command);
            assert_eq!(frame[..5], [verb, 0x44, 0x33, 0x22, 0x11]);
            assert_eq!(frame[5..], address);
            assert_eq!(decode(frame), Ok((adapter, command)));
        }
    }

    #[test]
    fn disconnect_cannot_downgrade_an_invalid_target_to_the_implicit_form() {
        let mut frame = encode(0, Command::Disconnect(None));
        frame[5..].fill(1);
        assert!(decode(frame).is_err());
        for address in [[0; 6], [255; 6]] {
            assert!(decode(encode(0, Command::Disconnect(Some(address)))).is_err());
        }
        assert!(parse(&["disconnect", "00:00:00:00:00:00"]).is_err());
        assert!(parse(&["disconnect", "FF:FF:FF:FF:FF:FF"]).is_err());
        assert!(parse(&["devices", "extra"]).is_err());
    }

    #[test]
    fn login_and_service_have_distinct_authority_requirements() {
        assert!(Peer::Caller.accepts(0, 0));
        assert!(!Peer::Service.accepts(0, 0));
        for peer in [Peer::Caller, Peer::Service] {
            assert!(peer.accepts(0, MANAGE));
            for flags in [0, MANAGE, CONFINED, MANAGE | CONFINED] {
                assert!(!peer.accepts(1000, flags));
            }
            for flags in [CONFINED, MANAGE | CONFINED, u32::MAX] {
                assert!(!peer.accepts(0, flags));
            }
        }
    }
}
