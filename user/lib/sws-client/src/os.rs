//! OS compatibility layer for SWS client.

use crate::Error;
use core::time::Duration;

#[cfg(feature = "std")]
pub use scarlet_os::handle::capability::memory_mapping::flags as mmap_flags;
#[cfg(feature = "std")]
pub use scarlet_os::ipc::permissions;
#[cfg(feature = "std")]
pub use scarlet_os::{SharedMemory, Socket};
#[cfg(feature = "std")]
pub use std::collections::BTreeMap;
#[cfg(feature = "std")]
pub use std::string::String;
#[cfg(feature = "std")]
pub use std::vec::Vec;

#[cfg(not(feature = "std"))]
pub use scarlet_std::collections::BTreeMap;
#[cfg(not(feature = "std"))]
pub use scarlet_std::handle::capability::memory_mapping::flags as mmap_flags;
#[cfg(not(feature = "std"))]
pub use scarlet_std::ipc::SharedMemory;
#[cfg(not(feature = "std"))]
pub use scarlet_std::ipc::permissions;
#[cfg(not(feature = "std"))]
pub use scarlet_std::socket::Socket;
#[cfg(not(feature = "std"))]
pub use scarlet_std::string::String;
#[cfg(not(feature = "std"))]
pub use scarlet_std::vec::Vec;

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
pub fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

#[cfg(not(feature = "std"))]
pub fn sleep(duration: Duration) {
    let _ = scarlet_std::thread::sleep(duration);
}
