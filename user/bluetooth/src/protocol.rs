//! Legacy clients receive text. Setting the high bit on a verb selects a typed JSON
//! success payload. Old services reject that verb before acting on it. Error payloads
//! remain UTF-8 in both cases. Never replay a command once submitted.
use crate::Result;
use alloc::{format, string::String, vec, vec::Vec};
use scarlet_abi::bluetooth as bt;
use scarlet_os::{
    handle::capability::{StreamError, StreamOps, StreamResult},
    socket::{Socket, SocketError},
};

pub const ENDPOINT: &str = "scarlet.bluetooth.v1";
pub const MAX_REPLY: usize = 128 * 1024;
const TYPED: u8 = 0x80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Status,
    Scan,
    Devices,
    Bonded,
    Pair(bt::Address),
    Connect(bt::Address),
    Disconnect(Option<bt::Address>),
}

pub fn credentials(socket: &Socket) -> Result<(u32, u32)> {
    let mut output = [0xff; 8];
    // SAFETY: the local socket control synchronously fills an eight-byte buffer.
    let rc = unsafe {
        socket.as_handle().control(
            scarlet_abi::SCTL_LOCAL_PEER_CREDENTIALS,
            output.as_mut_ptr() as usize,
        )
    }
    .map_err(|e| format!("cannot authenticate Bluetooth IPC peer ({e:?})"))?;
    let principal = u32::from_le_bytes(output[..4].try_into().unwrap());
    let flags = u32::from_le_bytes(output[4..].try_into().unwrap());
    if rc != 0 {
        return Err("invalid Bluetooth peer credentials".into());
    }
    Ok((principal, flags))
}

pub fn timeouts(socket: &Socket, read_ms: usize) -> Result<()> {
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
pub fn encode(adapter: u32, command: Command) -> [u8; 11] {
    let (verb, peer) = match command {
        Command::Status => (1, [0; 6]),
        Command::Scan => (2, [0; 6]),
        Command::Pair(peer) => (3, peer),
        Command::Connect(peer) => (4, peer),
        Command::Disconnect(None) => (5, [0; 6]),
        Command::Devices => (6, [0; 6]),
        Command::Bonded => (8, [0; 6]),
        Command::Disconnect(Some(peer)) => (7, peer),
    };
    let mut request = [0; 11];
    request[0] = verb;
    request[1..5].copy_from_slice(&adapter.to_le_bytes());
    request[5..].copy_from_slice(&peer);
    request
}

pub fn decode(request: [u8; 11]) -> Result<(u32, Command, bool)> {
    let peer = request[5..].try_into().unwrap();
    let command = match request[0] & !TYPED {
        1 if peer == [0; 6] => Command::Status,
        2 if peer == [0; 6] => Command::Scan,
        3 if peer != [0; 6] && peer != [255; 6] => Command::Pair(peer),
        4 if peer != [0; 6] && peer != [255; 6] => Command::Connect(peer),
        5 if peer == [0; 6] => Command::Disconnect(None),
        6 if peer == [0; 6] => Command::Devices,
        8 if peer == [0; 6] => Command::Bonded,
        7 if peer != [0; 6] && peer != [255; 6] => Command::Disconnect(Some(peer)),
        _ => return Err("invalid Bluetooth service request".into()),
    };
    let adapter = u32::from_le_bytes(request[1..5].try_into().unwrap());
    Ok((adapter, command, request[0] & TYPED != 0))
}

pub fn open() -> core::result::Result<Socket, SocketError> {
    let socket = Socket::new()?;
    socket.connect_abstract(ENDPOINT)?;
    Ok(socket)
}

pub struct Stream<'a>(StreamOps<'a>);

impl<'a> Stream<'a> {
    pub fn new(socket: &'a Socket) -> Result<Self> {
        socket
            .as_stream()
            .map(Self)
            .map_err(|e| format!("Bluetooth stream: {e:?}"))
    }

    pub fn read_exact(&self, bytes: &mut [u8]) -> Result<()> {
        transfer(bytes.len(), |offset| self.0.read(&mut bytes[offset..]))
    }

    pub fn write_all(&self, bytes: &[u8]) -> Result<()> {
        transfer(bytes.len(), |offset| self.0.write(&bytes[offset..]))
    }
}

// A signal can interrupt a short read or write anywhere in a frame. Resume
// at the first untransferred byte, including for a mutating request.
fn transfer(size: usize, mut next: impl FnMut(usize) -> StreamResult<usize>) -> Result<()> {
    let mut offset = 0;
    while offset < size {
        match next(offset) {
            Ok(0) => return Err("Bluetooth service stream closed".into()),
            Ok(count) if count <= size - offset => offset += count,
            Ok(_) => return Err("invalid Bluetooth stream length".into()),
            Err(StreamError::Interrupted) => continue,
            Err(error) => return Err(format!("Bluetooth service I/O failed ({error:?})")),
        }
    }
    Ok(())
}

fn is_service(credentials: (u32, u32)) -> bool {
    credentials == (0, scarlet_abi::LOCAL_PEER_MANAGE_DEVICES)
}

pub fn exchange(socket: &Socket, adapter: u32, command: Command, typed: bool) -> Result<Vec<u8>> {
    if !is_service(credentials(socket)?) {
        return Err("Bluetooth endpoint does not belong to a privileged system service".into());
    }
    timeouts(socket, 130_000)?;
    let stream = Stream::new(socket)?;
    let mut allowed = [0];
    stream.read_exact(&mut allowed)?;
    if allowed != [1] {
        return Err("Bluetooth management requires an unconfined login account".into());
    }
    let mut request = encode(adapter, command);
    if typed {
        request[0] |= TYPED;
    }
    stream.write_all(&request)?;
    let mut header = [0; 5];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
    if header[0] > 1 || length > MAX_REPLY {
        return Err("invalid Bluetooth service reply".into());
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    if header[0] == 0 {
        Ok(bytes)
    } else {
        Err(String::from_utf8(bytes).unwrap_or_else(|_| "invalid Bluetooth service error".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_abi::{LOCAL_PEER_CONFINED as CONFINED, LOCAL_PEER_MANAGE_DEVICES as MANAGE};

    #[test]
    fn only_unconfined_root_with_device_authority_authenticates_as_service() {
        assert!(is_service((0, MANAGE)));
        for uid in [0, 1001, u32::MAX] {
            for flags in [0, CONFINED, CONFINED | MANAGE, u32::MAX] {
                assert!(!is_service((uid, flags)));
            }
        }
        assert!(!is_service((1001, MANAGE)));
    }

    #[test]
    fn interrupted_fragment_resumes_without_replaying_the_prefix() {
        let mut steps = [(0, Ok(2)), (2, Err(StreamError::Interrupted)), (2, Ok(1))].into_iter();
        transfer(3, |offset| {
            let (expected, result) = steps.next().unwrap();
            assert_eq!(offset, expected);
            result
        })
        .unwrap();
        assert!(steps.next().is_none());
        assert!(transfer(1, |_| Ok(0)).is_err());
    }
}
