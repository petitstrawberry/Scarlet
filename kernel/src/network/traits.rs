use core::fmt::Debug;

use alloc::string::String;
use alloc::boxed::Box;

use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;

/// Next action instruction for pipeline processing
#[derive(Debug, Clone, PartialEq)]
pub enum NextAction {
    /// Jump to the specified stage
    JumpTo(String),
    /// Complete pipeline processing
    Complete,
}

/// Unified receive handler trait (each handler determines next stage internally)
pub trait ReceiveHandler: Send + Sync + Debug {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError>;
}

/// Unified transmit handler trait (each handler determines next stage internally)
pub trait TransmitHandler: Send + Sync + Debug {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError>;
}

/// Generic next stage matcher trait (type-safe)
pub trait NextStageMatcher<T>: Send + Sync + Debug {
    fn get_next_stage(&self, value: T) -> Result<&str, NetworkError>;
}
