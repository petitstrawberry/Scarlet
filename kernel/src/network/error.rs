use alloc::string::String;

/// Network-related error types
#[derive(Debug, Clone, PartialEq)]
pub enum NetworkError {
    /// Unsupported protocol
    UnsupportedProtocol {
        layer: String,
        protocol: String,
    },
    /// Stage not found
    StageNotFound(String),
    /// Receive handler does not exist
    NoRxHandler(String),
    /// Transmit handler does not exist
    NoTxHandler(String),
    /// Invalid operation
    InvalidOperation(String),
    /// Insufficient payload size
    InsufficientPayloadSize {
        required: usize,
        actual: usize,
    },
    /// Hints not found
    MissingHint(String),
    /// Invalid hints format
    InvalidHintFormat {
        hint_name: String,
        value: String,
    },
}

impl NetworkError {
    pub fn unsupported_protocol(layer: &str, protocol: &str) -> Self {
        Self::UnsupportedProtocol {
            layer: String::from(layer),
            protocol: String::from(protocol),
        }
    }

    pub fn stage_not_found(stage: &str) -> Self {
        Self::StageNotFound(String::from(stage))
    }

    pub fn no_rx_handler(stage: &str) -> Self {
        Self::NoRxHandler(String::from(stage))
    }

    pub fn no_tx_handler(stage: &str) -> Self {
        Self::NoTxHandler(String::from(stage))
    }

    pub fn invalid_operation(message: &str) -> Self {
        Self::InvalidOperation(String::from(message))
    }

    pub fn insufficient_payload_size(required: usize, actual: usize) -> Self {
        Self::InsufficientPayloadSize { required, actual }
    }

    pub fn missing_hint(hint_name: &str) -> Self {
        Self::MissingHint(String::from(hint_name))
    }

    pub fn invalid_hint_format(hint_name: &str, value: &str) -> Self {
        Self::InvalidHintFormat {
            hint_name: String::from(hint_name),
            value: String::from(value),
        }
    }
}
