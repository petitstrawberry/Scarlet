//! SAS client connection management.

use crate::error::Error;
use crate::os::{self, SharedMemory, Socket, Vec};
use crate::stream::{SasStream, StreamConfig};
use sas_protocol as protocol;

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
        let proto_config = protocol::Config {
            format: config.format,
            rate: config.rate,
            channels: config.channels,
            reserved: 0,
            period_frames: config.period_frames,
            buffer_frames: config.buffer_frames,
        };

        // Send MSG_CONFIGURE.
        let frame = protocol::frame(protocol::MSG_CONFIGURE, &proto_config.to_le_bytes());
        write_all(&mut self.socket, &frame)?;

        // Read response.
        read_ok(&mut self.socket)?;

        // Receive SHM handle out-of-band.
        let shm_handle = self
            .socket
            .recv_handle()
            .map_err(|_| Error::ShmHandleFailed)?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;

        // Map the shared ring.
        let frame_bytes = config.channels as usize * 2; // S16LE
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

        Ok(SasStream::new(ring_addr, ring_size, config))
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
