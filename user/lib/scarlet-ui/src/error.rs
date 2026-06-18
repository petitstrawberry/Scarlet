//! Error types for ScarletUI

use core::fmt;

/// ScarletUI error types
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Invalid window size
    InvalidSize { width: u32, height: u32 },

    /// Window creation failed
    WindowCreationFailed,

    /// Surface creation failed
    SurfaceCreationFailed,

    /// Connection to window server failed
    ConnectionFailed,

    /// IO error
    IoError,

    /// Invalid state ID
    InvalidStateId,

    /// Layout constraint violation
    LayoutConstraintViolation,

    /// Rendering error
    RenderError,

    /// Event dispatch error
    EventDispatchError,

    /// Duplicate scene window key
    DuplicateSceneWindowKey,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidSize { width, height } => {
                write!(f, "Invalid window size: {}x{}", width, height)
            }
            Error::WindowCreationFailed => write!(f, "Failed to create window"),
            Error::SurfaceCreationFailed => write!(f, "Failed to create surface"),
            Error::ConnectionFailed => write!(f, "Failed to connect to window server"),
            Error::IoError => write!(f, "IO error"),
            Error::InvalidStateId => write!(f, "Invalid state ID"),
            Error::LayoutConstraintViolation => write!(f, "Layout constraint violation"),
            Error::RenderError => write!(f, "Rendering error"),
            Error::EventDispatchError => write!(f, "Event dispatch error"),
            Error::DuplicateSceneWindowKey => write!(f, "Duplicate scene window key"),
        }
    }
}

/// Result type for ScarletUI operations
pub type Result<T> = core::result::Result<T, Error>;
