//! OS compatibility layer for SWS client.

use crate::Error;

#[cfg(feature = "std")]
pub use scarlet_os::Handle;
#[cfg(feature = "std")]
pub use scarlet_os::handle::capability::memory_mapping::flags as mmap_flags;
#[cfg(feature = "std")]
pub use scarlet_os::handle::capability::memory_mapping::munmap;
#[cfg(feature = "std")]
pub use scarlet_os::ipc::permissions;
#[cfg(feature = "std")]
pub use scarlet_os::{SharedMemory, Socket};
#[cfg(feature = "std")]
pub use std::collections::BTreeMap;
#[cfg(feature = "std")]
pub use std::string::String;
#[cfg(feature = "std")]
pub use std::sync::{Arc, Mutex, MutexGuard};
#[cfg(feature = "std")]
pub use std::vec::Vec;

#[cfg(not(feature = "std"))]
pub use scarlet_std::collections::BTreeMap;
#[cfg(not(feature = "std"))]
pub use scarlet_std::handle::Handle;
#[cfg(not(feature = "std"))]
pub use scarlet_std::handle::capability::memory_mapping::flags as mmap_flags;
#[cfg(not(feature = "std"))]
pub use scarlet_std::handle::capability::memory_mapping::munmap;
#[cfg(not(feature = "std"))]
pub use scarlet_std::ipc::SharedMemory;
#[cfg(not(feature = "std"))]
pub use scarlet_std::ipc::permissions;
#[cfg(not(feature = "std"))]
pub use scarlet_std::socket::Socket;
#[cfg(not(feature = "std"))]
pub use scarlet_std::string::String;
#[cfg(not(feature = "std"))]
pub use scarlet_std::sync::{Arc, Mutex, MutexGuard};
#[cfg(not(feature = "std"))]
pub use scarlet_std::vec::Vec;

#[cfg(feature = "std")]
pub fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(not(feature = "std"))]
pub fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock()
}

#[cfg(feature = "std")]
pub fn socket_read(socket: &mut Socket, buf: &mut [u8]) -> Result<usize, Error> {
    use scarlet_os::handle::capability::StreamError;

    match socket.as_stream().map_err(|_| Error::IoError)?.read(buf) {
        Ok(0) => Err(Error::Disconnected),
        Ok(n) => Ok(n),
        Err(StreamError::WouldBlock) => Err(Error::WouldBlock),
        Err(StreamError::EndOfStream) => Err(Error::Disconnected),
        Err(_) => Err(Error::IoError),
    }
}

#[cfg(not(feature = "std"))]
pub fn socket_read(socket: &mut Socket, buf: &mut [u8]) -> Result<usize, Error> {
    use scarlet_std::io::{ErrorKind, Read};

    match socket.read(buf) {
        Ok(0) => Err(Error::Disconnected),
        Ok(n) => Ok(n),
        Err(e) if e.kind() == ErrorKind::WouldBlock => Err(Error::WouldBlock),
        Err(_) => Err(Error::IoError),
    }
}

#[cfg(feature = "std")]
pub fn socket_write(socket: &mut Socket, buf: &[u8]) -> Result<usize, Error> {
    use scarlet_os::handle::capability::StreamError;

    match socket.as_stream().map_err(|_| Error::IoError)?.write(buf) {
        Ok(0) => Err(Error::Disconnected),
        Ok(n) => Ok(n),
        Err(StreamError::WouldBlock) => Err(Error::WouldBlock),
        Err(_) => Err(Error::IoError),
    }
}

#[cfg(not(feature = "std"))]
pub fn socket_write(socket: &mut Socket, buf: &[u8]) -> Result<usize, Error> {
    use scarlet_std::io::{ErrorKind, Write};

    match socket.write(buf) {
        Ok(0) => Err(Error::Disconnected),
        Ok(n) => Ok(n),
        Err(e) if e.kind() == ErrorKind::WouldBlock => Err(Error::WouldBlock),
        Err(_) => Err(Error::IoError),
    }
}

#[cfg(feature = "std")]
pub fn socket_flush(_socket: &mut Socket) -> Result<(), Error> {
    Ok(())
}

#[cfg(not(feature = "std"))]
pub fn socket_flush(socket: &mut Socket) -> Result<(), Error> {
    use scarlet_std::io::Write;

    socket.flush().map_err(|_| Error::IoError)
}

#[cfg(feature = "std")]
pub fn socket_send_handle_and_data(
    socket: &Socket,
    handle: &Handle,
    data: &[u8],
) -> Result<(), Error> {
    socket
        .send_handle_and_data(handle, data)
        .map_err(|_| Error::SendFailed)
}

#[cfg(not(feature = "std"))]
pub fn socket_send_handle_and_data(
    socket: &Socket,
    handle: &Handle,
    data: &[u8],
) -> Result<(), Error> {
    socket
        .send_handle_and_data(handle, data)
        .map_err(|_| Error::SendFailed)
}

#[cfg(feature = "std")]
pub fn socket_recv_handle_and_data(
    socket: &Socket,
    data: &mut [u8],
) -> Result<(Handle, usize), Error> {
    use scarlet_os::socket::SocketError;

    match socket.recv_handle_and_data(data) {
        Ok(record) => Ok(record),
        Err(SocketError::WouldBlock) => Err(Error::WouldBlock),
        Err(SocketError::ReceiveBufferTooSmall { required_len }) => {
            Err(Error::ReceiveBufferTooSmall { required_len })
        }
        Err(_) => Err(Error::ReceiveFailed),
    }
}

#[cfg(not(feature = "std"))]
pub fn socket_recv_handle_and_data(
    socket: &Socket,
    data: &mut [u8],
) -> Result<(Handle, usize), Error> {
    use scarlet_std::socket::SocketError;

    match socket.recv_handle_and_data(data) {
        Ok(record) => Ok(record),
        Err(SocketError::WouldBlock) => Err(Error::WouldBlock),
        Err(SocketError::ReceiveBufferTooSmall { required_len }) => {
            Err(Error::ReceiveBufferTooSmall { required_len })
        }
        Err(_) => Err(Error::ReceiveFailed),
    }
}

#[cfg(feature = "std")]
pub fn yield_now() {
    std::thread::yield_now();
}

#[cfg(not(feature = "std"))]
pub fn yield_now() {
    scarlet_std::thread::yield_now();
}
