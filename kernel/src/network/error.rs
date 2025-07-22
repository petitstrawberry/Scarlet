//! Network pipeline error definitions
//!
//! Defines error types used throughout the network pipeline infrastructure.

/// Network pipeline error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    /// Stage not found in pipeline
    StageNotFound(alloc::string::String),
    /// Packet data insufficient for processing
    InsufficientData { 
        required: usize, 
        available: usize 
    },
    /// Invalid packet format
    InvalidPacket(alloc::string::String),
    /// Stage processing failed
    StageProcessingFailed {
        stage: alloc::string::String,
        reason: alloc::string::String,
    },
    /// No matching processor found in stage
    NoMatchingProcessor(alloc::string::String),
    /// Circular dependency detected in pipeline
    CircularDependency(alloc::string::String),
    /// Invalid stage configuration
    InvalidStageConfig(alloc::string::String),
    /// Pipeline not initialized
    PipelineNotInitialized,
    /// Missing required hint for transmit path processing
    MissingHint(alloc::string::String),
    /// Operation not supported
    Unsupported(alloc::string::String),
    /// Unsupported protocol encountered
    UnsupportedProtocol { 
        layer: alloc::string::String, 
        protocol: alloc::string::String 
    },
    /// Invalid hint format
    InvalidHintFormat { 
        hint_key: alloc::string::String, 
        value: alloc::string::String 
    },
    /// No receive handler available for stage
    NoRxHandler(alloc::string::String),
    /// No transmit handler available for stage
    NoTxHandler(alloc::string::String),
    /// Invalid operation
    InvalidOperation(alloc::string::String),
}

impl NetworkError {
    /// Create a stage not found error
    pub fn stage_not_found(stage: &str) -> Self {
        Self::StageNotFound(alloc::string::String::from(stage))
    }

    /// Create an insufficient data error
    pub fn insufficient_data(required: usize, available: usize) -> Self {
        Self::InsufficientData { required, available }
    }

    /// Create an invalid packet error
    pub fn invalid_packet(reason: &str) -> Self {
        Self::InvalidPacket(alloc::string::String::from(reason))
    }

    /// Create a stage processing failed error
    pub fn stage_processing_failed(stage: &str, reason: &str) -> Self {
        Self::StageProcessingFailed {
            stage: alloc::string::String::from(stage),
            reason: alloc::string::String::from(reason),
        }
    }

    /// Create a no matching processor error
    pub fn no_matching_processor(stage: &str) -> Self {
        Self::NoMatchingProcessor(alloc::string::String::from(stage))
    }

    /// Create a circular dependency error
    pub fn circular_dependency(description: &str) -> Self {
        Self::CircularDependency(alloc::string::String::from(description))
    }

    /// Create an invalid stage config error
    pub fn invalid_stage_config(reason: &str) -> Self {
        Self::InvalidStageConfig(alloc::string::String::from(reason))
    }

    /// Create a missing hint error
    pub fn missing_hint(hint_key: &str) -> Self {
        Self::MissingHint(alloc::string::String::from(hint_key))
    }

    /// Create an unsupported operation error
    pub fn unsupported(operation: &str) -> Self {
        Self::Unsupported(alloc::string::String::from(operation))
    }

    /// Create an unsupported protocol error
    pub fn unsupported_protocol(layer: &str, protocol: &str) -> Self {
        Self::UnsupportedProtocol {
            layer: alloc::string::String::from(layer),
            protocol: alloc::string::String::from(protocol),
        }
    }

    /// Create an invalid hint format error
    pub fn invalid_hint_format(hint_key: &str, value: &str) -> Self {
        Self::InvalidHintFormat {
            hint_key: alloc::string::String::from(hint_key),
            value: alloc::string::String::from(value),
        }
    }

    /// Create a no rx handler error
    pub fn no_rx_handler(stage: &str) -> Self {
        Self::NoRxHandler(alloc::string::String::from(stage))
    }

    /// Create a no tx handler error
    pub fn no_tx_handler(stage: &str) -> Self {
        Self::NoTxHandler(alloc::string::String::from(stage))
    }

    /// Create an invalid operation error
    pub fn invalid_operation(reason: &str) -> Self {
        Self::InvalidOperation(alloc::string::String::from(reason))
    }

    /// Get a static string description of the error
    pub fn as_str(&self) -> &str {
        match self {
            NetworkError::StageNotFound(_) => "Stage not found",
            NetworkError::InsufficientData { .. } => "Insufficient packet data",
            NetworkError::InvalidPacket(_) => "Invalid packet format",
            NetworkError::StageProcessingFailed { .. } => "Stage processing failed",
            NetworkError::NoMatchingProcessor(_) => "No matching processor found",
            NetworkError::CircularDependency(_) => "Circular dependency detected",
            NetworkError::InvalidStageConfig(_) => "Invalid stage configuration",
            NetworkError::PipelineNotInitialized => "Pipeline not initialized",
            NetworkError::MissingHint(_) => "Missing required hint",
            NetworkError::Unsupported(_) => "Operation not supported",
            NetworkError::UnsupportedProtocol { .. } => "Unsupported protocol",
            NetworkError::InvalidHintFormat { .. } => "Invalid hint format",
            NetworkError::NoRxHandler(_) => "No receive handler available",
            NetworkError::NoTxHandler(_) => "No transmit handler available",
            NetworkError::InvalidOperation(_) => "Invalid operation",
        }
    }
}

impl core::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NetworkError::StageNotFound(stage) => {
                write!(f, "Stage '{}' not found in pipeline", stage)
            }
            NetworkError::InsufficientData { required, available } => {
                write!(f, "Insufficient packet data: required {}, available {}", required, available)
            }
            NetworkError::InvalidPacket(reason) => {
                write!(f, "Invalid packet: {}", reason)
            }
            NetworkError::StageProcessingFailed { stage, reason } => {
                write!(f, "Stage '{}' processing failed: {}", stage, reason)
            }
            NetworkError::NoMatchingProcessor(stage) => {
                write!(f, "No matching processor found in stage '{}'", stage)
            }
            NetworkError::CircularDependency(description) => {
                write!(f, "Circular dependency detected: {}", description)
            }
            NetworkError::InvalidStageConfig(reason) => {
                write!(f, "Invalid stage configuration: {}", reason)
            }
            NetworkError::PipelineNotInitialized => {
                write!(f, "Pipeline not initialized")
            }
            NetworkError::MissingHint(hint_key) => {
                write!(f, "Missing required hint: {}", hint_key)
            }
            NetworkError::Unsupported(operation) => {
                write!(f, "Operation not supported: {}", operation)
            }
            NetworkError::UnsupportedProtocol { layer, protocol } => {
                write!(f, "Unsupported protocol in {}: {}", layer, protocol)
            }
            NetworkError::InvalidHintFormat { hint_key, value } => {
                write!(f, "Invalid format for hint '{}': {}", hint_key, value)
            }
            NetworkError::NoRxHandler(stage) => {
                write!(f, "No receive handler available for stage '{}'", stage)
            }
            NetworkError::NoTxHandler(stage) => {
                write!(f, "No transmit handler available for stage '{}'", stage)
            }
            NetworkError::InvalidOperation(reason) => {
                write!(f, "Invalid operation: {}", reason)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test_case]
    fn test_error_creation() {
        let err = NetworkError::stage_not_found("ethernet");
        assert_eq!(err, NetworkError::StageNotFound(String::from("ethernet")));
        assert_eq!(err.as_str(), "Stage not found");

        let err = NetworkError::insufficient_data(20, 14);
        assert_eq!(err, NetworkError::InsufficientData { required: 20, available: 14 });
        assert_eq!(err.as_str(), "Insufficient packet data");
    }

    #[test_case]
    fn test_error_display() {
        let err = NetworkError::stage_not_found("ipv4");
        let display = alloc::format!("{}", err);
        assert_eq!(display, "Stage 'ipv4' not found in pipeline");

        let err = NetworkError::insufficient_data(14, 10);
        let display = alloc::format!("{}", err);
        assert_eq!(display, "Insufficient packet data: required 14, available 10");
    }

    #[test_case]
    fn test_error_debug() {
        let err = NetworkError::invalid_packet("malformed header");
        // Just test that debug formatting works without panicking
        let _ = alloc::format!("{:?}", err);
    }
}