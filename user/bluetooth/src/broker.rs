//! One request per connected stream. Both ends authenticate the kernel's immutable
//! peer snapshot before exchanging commands. Only the service opens a numbered
//! adapter and translates the five public verbs to device controls.
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
        Command::Disconnect => (5, [0; 6]),
    };
    let mut request = [0; 11];
    request[0] = verb;
    request[1..5].copy_from_slice(&adapter.to_le_bytes());
    request[5..].copy_from_slice(&peer);
    request
}

fn dispatch(request: [u8; 11]) -> Result<String> {
    let peer = request[5..].try_into().unwrap();
    let command = match request[0] {
        1 if peer == [0; 6] => Command::Status,
        2 if peer == [0; 6] => Command::Scan,
        3 if peer != [0; 6] && peer != [255; 6] => Command::Pair(peer),
        4 if peer != [0; 6] && peer != [255; 6] => Command::Connect(peer),
        5 if peer == [0; 6] => Command::Disconnect,
        _ => return Err("invalid Bluetooth service request".into()),
    };
    let adapter = u32::from_le_bytes(request[1..5].try_into().unwrap());
    direct(&format!("/dev/bluetooth{adapter}"), command)
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
    let (status, message) = match dispatch(request) {
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
    use super::Peer;
    use scarlet_abi::{LOCAL_PEER_CONFINED as CONFINED, LOCAL_PEER_MANAGE_DEVICES as MANAGE};

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
