//! Error types for SAS client.

const SERVER_ERROR_LEN: usize = 128;

/// Error type for SAS client operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Failed to create socket.
    SocketCreation,
    /// Failed to connect to SAS server.
    ConnectionFailed,
    /// Failed to configure socket.
    SocketConfig,
    /// I/O operation would block (non-blocking mode).
    WouldBlock,
    /// Connection closed by remote.
    Disconnected,
    /// General I/O error.
    IoError,
    /// Failed to send message.
    SendFailed,
    /// Failed to receive message.
    ReceiveFailed,
    /// Invalid server response.
    InvalidResponse,
    /// Failed to receive shared memory handle.
    ShmHandleFailed,
    /// Failed to map shared memory ring buffer.
    RingMapFailed,
    /// Stream is not configured.
    NotConfigured,
    /// Protocol error.
    ProtocolError,
    /// SAS server rejected the request.
    ServerError {
        message: [u8; SERVER_ERROR_LEN],
        len: usize,
    },
}

impl Error {
    pub(crate) fn server_error(payload: &[u8]) -> Self {
        let mut message = [0u8; SERVER_ERROR_LEN];
        let len = payload.len().min(message.len().saturating_sub(1));
        message[..len].copy_from_slice(&payload[..len]);
        Self::ServerError { message, len }
    }

    /// Get a human-readable description of the error.
    pub fn as_str(&self) -> &str {
        match self {
            Error::SocketCreation => "failed to create socket",
            Error::ConnectionFailed => "failed to connect to SAS server",
            Error::SocketConfig => "failed to configure socket",
            Error::WouldBlock => "operation would block",
            Error::Disconnected => "connection closed",
            Error::IoError => "I/O error",
            Error::SendFailed => "failed to send message",
            Error::ReceiveFailed => "failed to receive message",
            Error::InvalidResponse => "invalid server response",
            Error::ShmHandleFailed => "failed to receive shared memory handle",
            Error::RingMapFailed => "failed to map shared memory ring",
            Error::NotConfigured => "stream is not configured",
            Error::ProtocolError => "protocol error",
            Error::ServerError { message, len } => {
                core::str::from_utf8(&message[..*len]).unwrap_or("server error")
            }
        }
    }
}
