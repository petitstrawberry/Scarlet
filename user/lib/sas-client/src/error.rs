//! Error types for SAS client.

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
}

impl Error {
    /// Get a human-readable description of the error.
    pub fn as_str(&self) -> &'static str {
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
        }
    }
}
