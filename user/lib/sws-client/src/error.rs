//! Error types for SWS client

/// Error type for SWS client operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Failed to create socket
    SocketCreation,
    /// Failed to connect to SWS server
    ConnectionFailed,
    /// Connection already established
    AlreadyConnected,
    /// Not connected to server
    NotConnected,
    /// Failed to set socket options
    SocketConfig,
    /// I/O operation would block (non-blocking mode)
    WouldBlock,
    /// Connection closed by remote
    Disconnected,
    /// General I/O error
    IoError,
    /// Failed to send message
    SendFailed,
    /// Failed to receive message
    ReceiveFailed,
    /// The destination is too small for the next atomic handle record
    ReceiveBufferTooSmall {
        /// Exact number of bytes required by the queued record
        required_len: usize,
    },
    /// Invalid server response
    InvalidResponse,
    /// Failed to receive shared memory handle
    ShmHandleFailed,
    /// Failed to map shared memory
    ShmMapFailed,
    /// Surface not found
    SurfaceNotFound,
    /// Protocol error
    ProtocolError,
    /// Invalid request (e.g., missing required field in builder)
    InvalidRequest,
    /// All non-zero request identifiers are currently in use
    RequestIdExhausted,
    /// Error response returned by SWS
    ServerError(u32),
}

impl Error {
    /// Get a human-readable description of the error
    pub fn as_str(&self) -> &'static str {
        match self {
            Error::SocketCreation => "failed to create socket",
            Error::ConnectionFailed => "failed to connect to SWS server",
            Error::AlreadyConnected => "already connected",
            Error::NotConnected => "not connected",
            Error::SocketConfig => "failed to configure socket",
            Error::WouldBlock => "operation would block",
            Error::Disconnected => "connection closed",
            Error::IoError => "I/O error",
            Error::SendFailed => "failed to send message",
            Error::ReceiveFailed => "failed to receive message",
            Error::ReceiveBufferTooSmall { .. } => "receive buffer is too small",
            Error::InvalidResponse => "invalid server response",
            Error::ShmHandleFailed => "failed to receive shared memory handle",
            Error::ShmMapFailed => "failed to map shared memory",
            Error::SurfaceNotFound => "surface not found",
            Error::ProtocolError => "protocol error",
            Error::InvalidRequest => "invalid request (missing required field)",
            Error::RequestIdExhausted => "all request identifiers are in use",
            Error::ServerError(_) => "server rejected the request",
        }
    }
}

impl From<sws_protocol::ProtocolError> for Error {
    fn from(e: sws_protocol::ProtocolError) -> Self {
        let _ = e;
        Error::ProtocolError
    }
}
