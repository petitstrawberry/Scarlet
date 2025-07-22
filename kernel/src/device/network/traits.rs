//! Traits for the network pipeline infrastructure
//!
//! This module defines the core traits that enable flexible packet processing
//! in the network pipeline architecture.

use super::{packet::NetworkPacket, error::NetworkError};

/// Trait for packet processing logic within a pipeline stage
///
/// StageHandler implementations perform the actual packet processing work:
/// - Parse and validate packet data
/// - Extract headers and store them in the packet
/// - Modify the payload for the next stage
/// - Perform protocol-specific operations
///
/// Handlers should NOT determine which stage to process next - that is the
/// responsibility of the NextAction associated with the StageProcessor.
pub trait StageHandler: Send + Sync {
    /// Process a packet within this stage
    ///
    /// The handler should:
    /// 1. Validate that the packet has sufficient data
    /// 2. Parse the protocol header from the current payload
    /// 3. Store the header using packet.add_header()
    /// 4. Update the payload to contain remaining data
    /// 5. Perform any protocol-specific processing
    ///
    /// # Arguments
    /// * `packet` - The packet to process (mutable to allow modifications)
    ///
    /// # Returns
    /// * `Ok(())` if processing succeeded
    /// * `Err(NetworkError)` if processing failed
    ///
    /// # Example
    /// ```ignore
    /// fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
    ///     // Validate minimum header size
    ///     packet.validate_payload_size(14)?;
    ///     
    ///     let payload = packet.payload();
    ///     
    ///     // Extract ethernet header (14 bytes)
    ///     packet.add_header("ethernet", payload[0..14].to_vec());
    ///     
    ///     // Set remaining payload
    ///     packet.set_payload(payload[14..].to_vec());
    ///     
    ///     Ok(())
    /// }
    /// ```
    fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError>;
}

/// Trait for determining if a processor should handle a packet
///
/// ProcessorCondition implementations check whether their associated
/// StageHandler should process a given packet. This allows multiple
/// processors in a single stage to handle different packet types.
///
/// Conditions should be fast and should NOT modify the packet.
pub trait ProcessorCondition: Send + Sync {
    /// Check if this processor's handler should process the packet
    ///
    /// This method should examine the packet data and return true if
    /// the associated StageHandler should process it. Common checks include:
    /// - Protocol type fields (EtherType, IP protocol, etc.)
    /// - Port numbers
    /// - Packet flags
    /// - Header presence
    ///
    /// # Arguments
    /// * `packet` - The packet to examine (read-only)
    ///
    /// # Returns
    /// * `true` if the associated handler should process this packet
    /// * `false` if this processor should be skipped
    ///
    /// # Example
    /// ```ignore
    /// fn matches(&self, packet: &NetworkPacket) -> bool {
    ///     let payload = packet.payload();
    ///     if payload.len() >= 14 {
    ///         // Check EtherType field for IPv4 (0x0800)
    ///         let ether_type = u16::from_be_bytes([payload[12], payload[13]]);
    ///         ether_type == 0x0800
    ///     } else {
    ///         false
    ///     }
    /// }
    /// ```
    fn matches(&self, packet: &NetworkPacket) -> bool;
}

/// Action to take after a stage processor completes
///
/// This enum defines what should happen next in the pipeline after
/// a StageProcessor successfully processes a packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    /// Jump to the specified stage for continued processing
    Jump(alloc::string::String),
    
    /// Processing is complete - deliver packet to application layer
    Complete,
    
    /// Drop the packet with the given reason
    Drop(alloc::string::String),
    
    /// Terminate processing temporarily (e.g., waiting for fragments)
    Terminate,
}

impl NextAction {
    /// Create a jump action to the specified stage
    pub fn jump_to(stage_name: &str) -> Self {
        Self::Jump(alloc::string::String::from(stage_name))
    }

    /// Create a drop action with the specified reason
    pub fn drop_with_reason(reason: &str) -> Self {
        Self::Drop(alloc::string::String::from(reason))
    }

    /// Check if this is a jump action
    pub fn is_jump(&self) -> bool {
        matches!(self, NextAction::Jump(_))
    }

    /// Check if this is a complete action
    pub fn is_complete(&self) -> bool {
        matches!(self, NextAction::Complete)
    }

    /// Check if this is a drop action
    pub fn is_drop(&self) -> bool {
        matches!(self, NextAction::Drop(_))
    }

    /// Check if this is a terminate action
    pub fn is_terminate(&self) -> bool {
        matches!(self, NextAction::Terminate)
    }

    /// Get the target stage name for Jump actions
    pub fn jump_target(&self) -> Option<&str> {
        match self {
            NextAction::Jump(stage) => Some(stage.as_str()),
            _ => None,
        }
    }

    /// Get the drop reason for Drop actions
    pub fn drop_reason(&self) -> Option<&str> {
        match self {
            NextAction::Drop(reason) => Some(reason.as_str()),
            _ => None,
        }
    }
}

impl core::fmt::Display for NextAction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NextAction::Jump(stage) => write!(f, "Jump({})", stage),
            NextAction::Complete => write!(f, "Complete"),
            NextAction::Drop(reason) => write!(f, "Drop({})", reason),
            NextAction::Terminate => write!(f, "Terminate"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{vec, string::String};

    // Mock implementations for testing

    struct MockHandler;

    impl StageHandler for MockHandler {
        fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            // Simple mock: just add a test header
            packet.add_header("test", vec![0x01, 0x02]);
            Ok(())
        }
    }

    struct MockCondition {
        should_match: bool,
    }

    impl ProcessorCondition for MockCondition {
        fn matches(&self, _packet: &NetworkPacket) -> bool {
            self.should_match
        }
    }

    struct FailingHandler;

    impl StageHandler for FailingHandler {
        fn handle(&self, _packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            Err(NetworkError::invalid_packet("Mock failure"))
        }
    }

    #[test_case]
    fn test_next_action_creation() {
        let action = NextAction::jump_to("ipv4");
        assert!(action.is_jump());
        assert!(!action.is_complete());
        assert_eq!(action.jump_target(), Some("ipv4"));

        let action = NextAction::drop_with_reason("invalid checksum");
        assert!(action.is_drop());
        assert_eq!(action.drop_reason(), Some("invalid checksum"));

        let action = NextAction::Complete;
        assert!(action.is_complete());
        assert!(!action.is_jump());

        let action = NextAction::Terminate;
        assert!(action.is_terminate());
    }

    #[test_case]
    fn test_next_action_display() {
        assert_eq!(NextAction::jump_to("tcp").to_string(), "Jump(tcp)");
        assert_eq!(NextAction::Complete.to_string(), "Complete");
        assert_eq!(NextAction::drop_with_reason("bad packet").to_string(), "Drop(bad packet)");
        assert_eq!(NextAction::Terminate.to_string(), "Terminate");
    }

    #[test_case]
    fn test_next_action_equality() {
        assert_eq!(NextAction::jump_to("ipv4"), NextAction::Jump(String::from("ipv4")));
        assert_eq!(NextAction::Complete, NextAction::Complete);
        assert_ne!(NextAction::Complete, NextAction::Terminate);
    }

    #[test_case]
    fn test_stage_handler_trait() {
        let handler = MockHandler;
        let mut packet = NetworkPacket::new(vec![0xAA, 0xBB], String::from("test"));

        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        assert_eq!(packet.get_header("test"), Some(&[0x01, 0x02][..]));
    }

    #[test_case]
    fn test_stage_handler_failure() {
        let handler = FailingHandler;
        let mut packet = NetworkPacket::new(vec![0xAA, 0xBB], String::from("test"));

        let result = handler.handle(&mut packet);
        assert!(result.is_err());
        match result {
            Err(NetworkError::InvalidPacket(msg)) => {
                assert_eq!(msg, "Mock failure");
            }
            _ => panic!("Expected InvalidPacket error"),
        }
    }

    #[test_case]
    fn test_processor_condition_trait() {
        let condition_true = MockCondition { should_match: true };
        let condition_false = MockCondition { should_match: false };
        let packet = NetworkPacket::new(vec![0xAA], String::from("test"));

        assert!(condition_true.matches(&packet));
        assert!(!condition_false.matches(&packet));
    }

    #[test_case]
    fn test_trait_object_send_sync() {
        // Test that we can create trait objects and they are Send + Sync
        let _handler: Box<dyn StageHandler> = Box::new(MockHandler);
        let _condition: Box<dyn ProcessorCondition> = Box::new(MockCondition { should_match: true });

        // This test mainly ensures the traits compile with Send + Sync bounds
    }
}