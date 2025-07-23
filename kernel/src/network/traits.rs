use core::fmt::Debug;

use alloc::string::String;
use alloc::boxed::Box;

use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;

/// Next action instruction for pipeline processing
#[derive(Debug, Clone, PartialEq)]
pub enum NextAction {
    /// Jump to the specified stage (same direction)
    JumpTo(String),
    /// Complete pipeline processing and send to device
    CompleteToDevice(String), // device name
    /// Complete pipeline processing and deliver to application
    CompleteToApplication,
    /// Drop the packet (no further processing)
    Drop,
    /// Change direction and jump to specified stage in opposite pipeline
    /// (e.g., from Rx to Tx for ICMP responses, ARP replies)
    ChangeDirection {
        stage: String,
        /// Optional new packet to use instead of current packet
        new_packet: Option<NetworkPacket>,
    },
    /// Spawn new packet in opposite direction while continuing current processing
    /// (useful for generating responses while processing original packet)
    SpawnInOppositeDirection {
        stage: String,
        new_packet: NetworkPacket,
        /// Continue processing current packet to this stage
        continue_to: Option<String>,
    },
}

/// Unified packet handler trait
pub trait PacketHandler: Send + Sync + Debug {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError>;
}

/// Generic next stage matcher trait (type-safe)
pub trait NextStageMatcher<T>: Send + Sync {
    fn get_next_stage(&self, value: T) -> Result<&str, NetworkError>;
}
