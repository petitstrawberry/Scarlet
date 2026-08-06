//! SAS client connection management.

use crate::error::Error;
use crate::os::{self, SharedMemory, Socket, Vec};
use crate::stream::{SasStream, StreamConfig};
use sas_protocol as protocol;

#[cfg(feature = "std")]
use scarlet_os::poll::{POLLERR, POLLHUP, POLLIN, POLLNVAL, PollHandle, poll};
#[cfg(feature = "std")]
use scarlet_os::socket::SocketError;
#[cfg(not(feature = "std"))]
use scarlet_std::poll::{POLLERR, POLLHUP, POLLIN, POLLNVAL, PollHandle, poll};
#[cfg(not(feature = "std"))]
use scarlet_std::socket::SocketError;

/// Read exactly `buf.len()` bytes from the socket.
fn read_exact(socket: &mut Socket, buf: &mut [u8]) -> Result<(), Error> {
    let mut filled = 0;
    while filled < buf.len() {
        match os::socket_read(socket, &mut buf[filled..]) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => filled += n,
            Err(Error::WouldBlock) => {
                os::sleep(core::time::Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Write all bytes to the socket.
fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), Error> {
    let mut written = 0;
    while written < bytes.len() {
        match os::socket_write(socket, &bytes[written..]) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => written += n,
            Err(Error::WouldBlock) => {
                os::sleep(core::time::Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    os::socket_flush(socket)
}

/// Read exactly `buf.len()` bytes while allowing the caller to cancel a
/// non-blocking control operation.
fn read_exact_cancellable<F>(
    socket: &mut Socket,
    buf: &mut [u8],
    should_cancel: &F,
) -> Result<Option<()>, Error>
where
    F: Fn() -> bool,
{
    let mut filled = 0;
    while filled < buf.len() {
        if should_cancel() {
            return Ok(None);
        }
        match os::socket_read(socket, &mut buf[filled..]) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => filled += n,
            Err(Error::WouldBlock) => {
                os::sleep(core::time::Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Some(()))
}

/// Write all bytes while allowing the caller to cancel a non-blocking control
/// operation.
fn write_all_cancellable<F>(
    socket: &mut Socket,
    bytes: &[u8],
    should_cancel: &F,
) -> Result<Option<()>, Error>
where
    F: Fn() -> bool,
{
    let mut written = 0;
    while written < bytes.len() {
        if should_cancel() {
            return Ok(None);
        }
        match os::socket_write(socket, &bytes[written..]) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => written += n,
            Err(Error::WouldBlock) => {
                os::sleep(core::time::Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    os::socket_flush(socket)?;
    Ok(Some(()))
}

/// Read one framed response and expect `MSG_OK`.
fn read_ok(socket: &mut Socket) -> Result<(), Error> {
    let mut header_bytes = [0u8; protocol::HEADER_SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::Header::from_le_bytes(header_bytes);

    let payload_len = header.payload_size as usize;
    if payload_len > protocol::MAX_PAYLOAD_SIZE {
        return Err(Error::ProtocolError);
    }

    let mut payload = Vec::new();
    payload.resize(payload_len, 0);
    if payload_len > 0 {
        read_exact(socket, &mut payload)?;
    }

    match header.msg_type {
        protocol::MSG_OK => Ok(()),
        protocol::MSG_ERROR => Err(Error::server_error(&payload)),
        _ => Err(Error::InvalidResponse),
    }
}

/// Read one framed response and expect `MSG_OK`, returning `None` if the
/// caller cancels while the response is pending.
fn read_ok_cancellable<F>(socket: &mut Socket, should_cancel: &F) -> Result<Option<()>, Error>
where
    F: Fn() -> bool,
{
    let mut header_bytes = [0u8; protocol::HEADER_SIZE];
    if read_exact_cancellable(socket, &mut header_bytes, should_cancel)?.is_none() {
        return Ok(None);
    }
    let header = protocol::Header::from_le_bytes(header_bytes);

    let payload_len = header.payload_size as usize;
    if payload_len > protocol::MAX_PAYLOAD_SIZE {
        return Err(Error::ProtocolError);
    }

    let mut payload = Vec::new();
    payload.resize(payload_len, 0);
    if payload_len > 0 && read_exact_cancellable(socket, &mut payload, should_cancel)?.is_none() {
        return Ok(None);
    }

    match header.msg_type {
        protocol::MSG_OK => Ok(Some(())),
        protocol::MSG_ERROR => Err(Error::server_error(&payload)),
        _ => Err(Error::InvalidResponse),
    }
}

/// Read one framed response and expect `MSG_CONTROL_STATE`.
fn read_control_state(socket: &mut Socket) -> Result<protocol::ControlState, Error> {
    let mut header_bytes = [0u8; protocol::HEADER_SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::Header::from_le_bytes(header_bytes);

    let payload_len = header.payload_size as usize;
    if payload_len > protocol::MAX_PAYLOAD_SIZE {
        return Err(Error::ProtocolError);
    }

    let mut payload = Vec::new();
    payload.resize(payload_len, 0);
    if payload_len > 0 {
        read_exact(socket, &mut payload)?;
    }

    match header.msg_type {
        protocol::MSG_CONTROL_STATE => {
            protocol::ControlState::from_payload(&payload).ok_or(Error::ProtocolError)
        }
        protocol::MSG_ERROR => Err(Error::server_error(&payload)),
        _ => Err(Error::InvalidResponse),
    }
}

/// Read one framed response and expect `MSG_OUTPUT_LIST`.
fn read_output_list(socket: &mut Socket) -> Result<Vec<protocol::OutputInfo>, Error> {
    let mut header_bytes = [0u8; protocol::HEADER_SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::Header::from_le_bytes(header_bytes);

    let payload_len = header.payload_size as usize;
    if payload_len > protocol::MAX_PAYLOAD_SIZE {
        return Err(Error::ProtocolError);
    }

    let mut payload = Vec::new();
    payload.resize(payload_len, 0);
    if payload_len > 0 {
        read_exact(socket, &mut payload)?;
    }

    match header.msg_type {
        protocol::MSG_OUTPUT_LIST => {
            protocol::output_list_from_payload(&payload).ok_or(Error::ProtocolError)
        }
        protocol::MSG_ERROR => Err(Error::server_error(&payload)),
        _ => Err(Error::InvalidResponse),
    }
}

/// Client connection to the Scarlet Audio Server.
///
/// A single `SasClient` manages the Unix domain socket connection.  Create one
/// with [`SasClient::connect`], then call [`SasClient::configure`] to obtain a
/// [`SasStream`] for writing PCM audio data.
pub struct SasClient {
    socket: Socket,
}

impl SasClient {
    /// Connect to SAS at the default socket path (`/tmp/sas.sock`).
    pub fn connect() -> Result<Self, Error> {
        Self::connect_to(protocol::SOCKET_PATH)
    }

    /// Connect to SAS at a custom socket path.
    pub fn connect_to(socket_path: &str) -> Result<Self, Error> {
        let socket = Socket::new().map_err(|_| Error::SocketCreation)?;
        socket
            .connect(socket_path)
            .map_err(|_| Error::ConnectionFailed)?;
        Ok(Self { socket })
    }

    /// Configure a stream and receive the shared ring buffer.
    ///
    /// Sends `MSG_CONFIGURE`, waits for `MSG_OK`, receives the SHM handle,
    /// maps it, and returns a [`SasStream`] ready for writing.
    pub fn configure(&mut self, config: &StreamConfig) -> Result<SasStream, Error> {
        self.configure_cancellable(config, || false)?
            .ok_or(Error::ReceiveFailed)
    }

    /// Configure a stream while periodically checking for cancellation.
    ///
    /// The control socket is temporarily switched to non-blocking mode so a
    /// stalled SAS server cannot keep the caller inside a read or write syscall.
    /// `Ok(None)` means that `should_cancel` became true. Since cancellation may
    /// leave a partially exchanged configure request on the control connection,
    /// callers should drop this `SasClient` after receiving `None`.
    pub fn configure_cancellable<F>(
        &mut self,
        config: &StreamConfig,
        should_cancel: F,
    ) -> Result<Option<SasStream>, Error>
    where
        F: Fn() -> bool,
    {
        if should_cancel() {
            return Ok(None);
        }

        let was_nonblocking = self
            .socket
            .is_nonblocking()
            .map_err(|_| Error::SocketConfig)?;
        if !was_nonblocking {
            self.socket
                .set_nonblocking(true)
                .map_err(|_| Error::SocketConfig)?;
        }

        let result = self.configure_cancellable_inner(config, &should_cancel);
        let restore_result = if was_nonblocking {
            Ok(())
        } else {
            self.socket
                .set_nonblocking(false)
                .map_err(|_| Error::SocketConfig)
        };

        match (result, restore_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(stream), Ok(())) => Ok(stream),
        }
    }

    fn configure_cancellable_inner<F>(
        &mut self,
        config: &StreamConfig,
        should_cancel: &F,
    ) -> Result<Option<SasStream>, Error>
    where
        F: Fn() -> bool,
    {
        let proto_config = protocol::Config {
            format: config.format,
            rate: config.rate,
            channels: config.channels,
            reserved: 0,
            period_frames: config.period_frames,
            buffer_frames: config.buffer_frames,
        };

        let frame = protocol::frame(protocol::MSG_CONFIGURE, &proto_config.to_le_bytes());
        if write_all_cancellable(&mut self.socket, &frame, should_cancel)?.is_none() {
            return Ok(None);
        }
        if read_ok_cancellable(&mut self.socket, should_cancel)?.is_none() {
            return Ok(None);
        }

        let shm_handle = loop {
            if should_cancel() {
                return Ok(None);
            }

            // SocketRecvHandle is a blocking syscall even when the stream is
            // marked non-blocking, so only enter it after poll reports a
            // queued record. A short poll interval bounds cancellation
            // latency without consuming CPU in a busy loop.
            let mut poll_handle = PollHandle::new(
                self.socket.as_raw() as u32,
                POLLIN | POLLERR | POLLHUP | POLLNVAL,
            );
            let ready = poll(core::slice::from_mut(&mut poll_handle), 1_000_000)
                .map_err(|_| Error::ReceiveFailed)?;
            if ready == 0 {
                continue;
            }

            if poll_handle.revents & POLLIN != 0 {
                match self.socket.recv_handle() {
                    Ok(handle) => break handle,
                    Err(SocketError::WouldBlock)
                        if poll_handle.revents & (POLLERR | POLLHUP) != 0 =>
                    {
                        return Err(Error::Disconnected);
                    }
                    Err(SocketError::WouldBlock) if poll_handle.revents & POLLNVAL != 0 => {
                        return Err(Error::ShmHandleFailed);
                    }
                    Err(SocketError::WouldBlock) => continue,
                    Err(_) => return Err(Error::ShmHandleFailed),
                }
            }
            if poll_handle.revents & (POLLERR | POLLHUP) != 0 {
                return Err(Error::Disconnected);
            }
            if poll_handle.revents & POLLNVAL != 0 {
                return Err(Error::ShmHandleFailed);
            }
        };
        if should_cancel() {
            return Ok(None);
        }
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;

        let frame_bytes = config.channels as usize * 2;
        let ring_size = protocol::RING_HEADER_SIZE + config.buffer_frames as usize * frame_bytes;
        let mapper = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| Error::RingMapFailed)?;
        let ring_addr = mapper
            .mmap(
                0,
                ring_size,
                os::prot::READ | os::prot::WRITE,
                os::mmap_flags::SHARED,
                0,
            )
            .map_err(|_| Error::RingMapFailed)?;
        let stream = SasStream::new(ring_addr, ring_size, config);
        if should_cancel() {
            return Ok(None);
        }

        Ok(Some(stream))
    }

    /// Query current SAS output control state.
    pub fn control_state(&mut self) -> Result<protocol::ControlState, Error> {
        let frame = protocol::frame(protocol::MSG_GET_CONTROL_STATE, &[]);
        write_all(&mut self.socket, &frame)?;
        read_control_state(&mut self.socket)
    }

    /// Set SAS master volume in unsigned Q16.16 fixed point.
    ///
    /// `sas_protocol::MASTER_VOLUME_UNITY_Q16` is unity gain. SAS rejects
    /// values above unity to keep the software mixer attenuating only.
    pub fn set_master_volume_q16(
        &mut self,
        master_volume_q16: u32,
    ) -> Result<protocol::ControlState, Error> {
        let payload = protocol::MasterVolume { master_volume_q16 }.to_le_bytes();
        let frame = protocol::frame(protocol::MSG_SET_MASTER_VOLUME, &payload);
        write_all(&mut self.socket, &frame)?;
        read_control_state(&mut self.socket)
    }

    /// Set SAS master mute state.
    pub fn set_master_muted(&mut self, muted: bool) -> Result<protocol::ControlState, Error> {
        let payload = protocol::MasterMute { muted }.to_le_bytes();
        let frame = protocol::frame(protocol::MSG_SET_MASTER_MUTE, &payload);
        write_all(&mut self.socket, &frame)?;
        read_control_state(&mut self.socket)
    }

    /// Switch SAS output device.
    pub fn set_output(
        &mut self,
        request: protocol::OutputRequest,
    ) -> Result<protocol::ControlState, Error> {
        let frame = protocol::frame(protocol::MSG_SET_OUTPUT, &request.to_le_bytes());
        write_all(&mut self.socket, &frame)?;
        read_control_state(&mut self.socket)
    }

    /// List SAS output devices.
    pub fn list_outputs(&mut self) -> Result<Vec<protocol::OutputInfo>, Error> {
        let frame = protocol::frame(protocol::MSG_LIST_OUTPUTS, &[]);
        write_all(&mut self.socket, &frame)?;
        read_output_list(&mut self.socket)
    }

    /// Send `MSG_DRAIN` and wait for `MSG_OK`.
    ///
    /// Blocks until the server has consumed all buffered audio.
    pub fn drain(&mut self) -> Result<(), Error> {
        let frame = protocol::frame(protocol::MSG_DRAIN, &[]);
        write_all(&mut self.socket, &frame)?;
        read_ok(&mut self.socket)
    }

    /// Send `MSG_CLOSE` to disconnect gracefully.
    pub fn close(&mut self) -> Result<(), Error> {
        let frame = protocol::frame(protocol::MSG_CLOSE, &[]);
        write_all(&mut self.socket, &frame)
    }
}
