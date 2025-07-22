//! Traits for the network pipeline infrastructure
//!
//! This module defines the core traits that enable flexible packet processing
//! in the Tx/Rx separated network pipeline architecture.

use super::{packet::NetworkPacket, error::NetworkError};

/// Trait for receive path packet processing logic within a pipeline stage
///
/// RxStageHandler implementations perform packet processing for received packets:
/// - Parse and validate packet data
/// - Extract headers and store them in the packet
/// - Modify the payload for the next stage
/// - Perform protocol-specific receive operations
///
/// Handlers should NOT determine which stage to process next - that is the
/// responsibility of the NextAction associated with the RxStageProcessor.
pub trait RxStageHandler: Send + Sync {
    /// Process a received packet within this stage
    ///
    /// The handler should:
    /// 1. Validate that the packet has sufficient data
    /// 2. Parse the protocol header from the current payload
    /// 3. Store the header using packet.add_header()
    /// 4. Update the payload to contain remaining data
    /// 5. Perform any protocol-specific receive processing
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

/// Trait for transmit path packet building logic within a pipeline stage
///
/// TxStageBuilder implementations perform packet building for transmission:
/// - Read hints from upper layers to determine configuration
/// - Build protocol headers based on hints and payload
/// - Prepend headers to payload data
/// - Set hints for lower layers
///
/// Builders should NOT determine which stage to process next - that is the
/// responsibility of the NextAction associated with the TxStageProcessor.
pub trait TxStageBuilder: Send + Sync {
    /// Build headers and modify packet for transmission
    ///
    /// The builder should:
    /// 1. Read required hints from the packet
    /// 2. Validate hints and payload for transmission
    /// 3. Build appropriate protocol header
    /// 4. Prepend header to payload data
    /// 5. Set hints for lower layers
    ///
    /// # Arguments
    /// * `packet` - The packet to build for (mutable to allow modifications)
    ///
    /// # Returns
    /// * `Ok(())` if building succeeded
    /// * `Err(NetworkError)` if building failed (e.g., missing hints)
    ///
    /// # Example
    /// ```ignore
    /// fn build(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
    ///     // Read hints from upper layer
    ///     let dest_ip = packet.get_hint("destination_ip")
    ///         .ok_or(NetworkError::missing_hint("destination_ip"))?;
    ///     
    ///     // Build IPv4 header
    ///     let header = self.build_ipv4_header(dest_ip, packet.payload().len())?;
    ///     
    ///     // Prepend header to payload
    ///     let mut new_payload = header;
    ///     new_payload.extend_from_slice(packet.payload());
    ///     packet.set_payload(new_payload);
    ///     
    ///     // Set hints for lower layer
    ///     packet.set_hint("link_layer", "ethernet");
    ///     
    ///     Ok(())
    /// }
    /// ```
    fn build(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError>;
}

/// Trait for determining if a processor should handle a packet
///
/// ProcessorCondition implementations check whether their associated
/// RxStageHandler or TxStageBuilder should process a given packet. This allows multiple
/// processors in a single stage to handle different packet types.
///
/// Conditions should be fast and should NOT modify the packet.
/// For receive path, conditions typically examine payload data.
/// For transmit path, conditions typically examine hints.
pub trait ProcessorCondition: Send + Sync {
    /// Check if this processor should handle the packet
    ///
    /// This method should examine the packet data (payload for Rx, hints for Tx) and
    /// return true if the associated handler/builder should process it. Common checks include:
    /// - Protocol type fields (EtherType, IP protocol, etc.) for receive path
    /// - Hints values (ip_version, protocol) for transmit path
    /// - Port numbers, packet flags, header presence
    ///
    /// # Arguments
    /// * `packet` - The packet to examine (read-only)
    ///
    /// # Returns
    /// * `true` if the associated handler/builder should process this packet
    /// * `false` if this processor should be skipped
    ///
    /// # Example for Receive Path
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
    ///
    /// # Example for Transmit Path
    /// ```ignore
    /// fn matches(&self, packet: &NetworkPacket) -> bool {
    ///     // Check if hints indicate IPv4 should be used
    ///     packet.get_hint("ip_version") == Some("ipv4")
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

// ===== Phase 1: Unified Handler Traits with Internal Routing =====

/// Unified trait for receive path packet processing with internal next stage determination
///
/// This trait implements the "1ステージ1ハンドラー設計" (1 stage, 1 handler design) where
/// each stage has exactly one receive handler that internally determines the next processing stage.
///
/// Key differences from RxStageHandler:
/// - Returns NextAction (including next stage) instead of just Ok(())
/// - Handler is responsible for routing decisions using NextStageMatcher
/// - Enables O(1) routing performance through HashMap-based matchers
pub trait ReceiveHandler: Send + Sync {
    /// Process a received packet and determine the next action
    ///
    /// The handler should:
    /// 1. Validate packet data (validate_payload_size, etc.)
    /// 2. Parse protocol headers from payload
    /// 3. Store headers using packet.add_header()
    /// 4. Update payload for next stage
    /// 5. Determine next stage using internal routing logic (NextStageMatcher)
    /// 6. Return appropriate NextAction
    ///
    /// # Arguments
    /// * `packet` - The packet to process (mutable for header extraction and payload updates)
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take next (JumpTo, Complete, Drop, Terminate)
    /// * `Err(NetworkError)` - Processing failed
    ///
    /// # Example
    /// ```ignore
    /// fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
    ///     packet.validate_payload_size(14)?;
    ///     let payload = packet.payload();
    ///     
    ///     // Extract Ethernet header
    ///     packet.add_header("ethernet", payload[0..14].to_vec());
    ///     packet.set_payload(payload[14..].to_vec());
    ///     
    ///     // Internal routing using O(1) HashMap lookup
    ///     let ether_type = u16::from_be_bytes([payload[12], payload[13]]);
    ///     let next_stage = self.next_stage_matcher.get_next_stage(ether_type)?;
    ///     Ok(NextAction::jump_to(next_stage))
    /// }
    /// ```
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError>;
}

/// Unified trait for transmit path packet building with internal next stage determination
///
/// This trait implements the "1ステージ1ハンドラー設計" (1 stage, 1 handler design) where
/// each stage has exactly one transmit handler that internally determines the next processing stage.
///
/// Key differences from TxStageBuilder:
/// - Returns NextAction (including next stage) instead of just Ok(())
/// - Handler is responsible for routing decisions
/// - Enables coordinated transmit path processing
pub trait TransmitHandler: Send + Sync {
    /// Build packet headers and determine the next action
    ///
    /// The builder should:
    /// 1. Read required hints from packet.get_hint()
    /// 2. Validate hints and payload for transmission
    /// 3. Build appropriate protocol header
    /// 4. Prepend header to payload (or modify packet accordingly)
    /// 5. Set hints for lower layers using packet.set_hint()
    /// 6. Determine next stage or completion
    /// 7. Return appropriate NextAction
    ///
    /// # Arguments
    /// * `packet` - The packet to build (mutable for header construction and hint updates)
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take next (JumpTo for next layer, Complete for transmission)
    /// * `Err(NetworkError)` - Building failed (missing hints, invalid configuration, etc.)
    ///
    /// # Example
    /// ```ignore
    /// fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
    ///     let dest_ip = packet.get_hint("destination_ip")
    ///         .ok_or(NetworkError::missing_hint("destination_ip"))?;
    ///     
    ///     // Build IPv4 header based on hints
    ///     let header = self.build_ipv4_header(dest_ip, packet.payload().len())?;
    ///     let mut new_payload = header;
    ///     new_payload.extend_from_slice(packet.payload());
    ///     packet.set_payload(new_payload);
    ///     
    ///     // Set hints for ethernet layer
    ///     packet.set_hint("ethertype", "0x0800");
    ///     
    ///     Ok(NextAction::jump_to("ethernet"))
    /// }
    /// ```
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError>;
}

/// Generic trait for O(1) next stage determination using HashMap routing
///
/// This trait enables type-safe, high-performance routing decisions within handlers
/// by providing O(1) lookup of next stages based on protocol-specific values.
///
/// Different protocol layers use different types for routing:
/// - Ethernet: u16 (EtherType)
/// - IPv4: u8 (IP protocol number)
/// - TCP/UDP: u16 (port numbers) or port ranges
pub trait NextStageMatcher<T>: Send + Sync {
    /// Get the next stage name for the given protocol value
    ///
    /// # Arguments
    /// * `value` - Protocol-specific routing value (EtherType, IP protocol, port, etc.)
    ///
    /// # Returns
    /// * `Ok(&str)` - Next stage name to jump to
    /// * `Err(NetworkError)` - No route found for this value
    ///
    /// # Example
    /// ```ignore
    /// let matcher = EtherTypeToStage::new()
    ///     .add_mapping(0x0800, "ipv4")
    ///     .add_mapping(0x86DD, "ipv6");
    ///     
    /// let next_stage = matcher.get_next_stage(0x0800)?; // Returns "ipv4"
    /// ```
    fn get_next_stage(&self, value: T) -> Result<&str, NetworkError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{boxed::Box, vec, string::String, string::ToString};

    // Mock implementations for testing

    struct MockRxHandler;

    impl RxStageHandler for MockRxHandler {
        fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            // Simple mock: just add a test header
            packet.add_header("test", vec![0x01, 0x02]);
            Ok(())
        }
    }

    struct MockTxBuilder;

    impl TxStageBuilder for MockTxBuilder {
        fn build(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            // Simple mock: prepend a test header to payload
            let mut new_payload = vec![0xAA, 0xBB]; // Mock header
            new_payload.extend_from_slice(packet.payload());
            packet.set_payload(new_payload);
            packet.set_hint("processed", "true");
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

    struct FailingRxHandler;

    impl RxStageHandler for FailingRxHandler {
        fn handle(&self, _packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            Err(NetworkError::invalid_packet("Mock failure"))
        }
    }

    struct FailingTxBuilder;

    impl TxStageBuilder for FailingTxBuilder {
        fn build(&self, _packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            Err(NetworkError::missing_hint("required_hint"))
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
    fn test_rx_stage_handler_trait() {
        let handler = MockRxHandler;
        let mut packet = NetworkPacket::new(vec![0xAA, 0xBB], String::from("test"));

        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        assert_eq!(packet.get_header("test"), Some(&[0x01, 0x02][..]));
    }

    #[test_case]
    fn test_tx_stage_builder_trait() {
        let builder = MockTxBuilder;
        let mut packet = NetworkPacket::new(vec![0xCC, 0xDD], String::from("test"));

        let result = builder.build(&mut packet);
        assert!(result.is_ok());
        assert_eq!(packet.payload(), &[0xAA, 0xBB, 0xCC, 0xDD]); // Header prepended
        assert_eq!(packet.get_hint("processed"), Some("true"));
    }

    #[test_case]
    fn test_rx_handler_failure() {
        let handler = FailingRxHandler;
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
    fn test_tx_builder_failure() {
        let builder = FailingTxBuilder;
        let mut packet = NetworkPacket::new(vec![0xAA, 0xBB], String::from("test"));

        let result = builder.build(&mut packet);
        assert!(result.is_err());
        match result {
            Err(NetworkError::MissingHint(hint)) => {
                assert_eq!(hint, "required_hint");
            }
            _ => panic!("Expected MissingHint error"),
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
        let _rx_handler: Box<dyn RxStageHandler> = Box::new(MockRxHandler);
        let _tx_builder: Box<dyn TxStageBuilder> = Box::new(MockTxBuilder);
        let _condition: Box<dyn ProcessorCondition> = Box::new(MockCondition { should_match: true });

        // This test mainly ensures the traits compile with Send + Sync bounds
    }
}