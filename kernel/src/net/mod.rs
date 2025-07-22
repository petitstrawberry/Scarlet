//! Network protocol stack
//!
//! This module implements the network protocol stack for Scarlet kernel,
//! featuring a flexible pipeline architecture with Tx/Rx separation.
//!
//! ## Architecture Overview
//!
//! The network stack is built around a flexible pipeline system that enables
//! protocol-independent packet processing:
//!
//! - **NetworkManager**: Orchestrates packet processing through pipelines
//! - **FlexiblePipeline**: Manages collections of processing stages
//! - **FlexibleStage**: Groups processors for specific protocol layers
//! - **RxStageProcessor/TxStageProcessor**: Tx/Rx separated packet processors
//! - **RxStageHandler/TxStageBuilder**: Protocol-specific packet handlers
//! - **ProcessorCondition**: Determines packet routing through stages
//!
//! ## Tx/Rx Separation
//!
//! The architecture separates receive and transmit paths:
//! - **Rx Path**: Parses headers and extracts protocol information
//! - **Tx Path**: Builds headers using hints from upper layers
//!
//! ## Packet Processing Flow
//!
//! ```text
//! Receive: Device → Ethernet → IPv4 → TCP/UDP → Application
//! Transmit: Application → TCP/UDP → IPv4 → Ethernet → Device
//! ```

// Core infrastructure
pub mod error;
pub mod packet;
pub mod traits;
pub mod pipeline;
pub mod network_manager;

// Examples and testing
pub mod examples;

// Protocol implementations (Phase 2+)
// pub mod protocol;

// Re-export key types for convenience
pub use error::NetworkError;
pub use packet::{NetworkPacket, Instant};
pub use traits::{RxStageHandler, TxStageBuilder, ProcessorCondition, NextAction};
pub use pipeline::{FlexiblePipeline, FlexibleStage, RxStageProcessor, TxStageProcessor};
pub use network_manager::{NetworkManager, NetworkProcessingStats};

/// Create a basic example network manager for testing
pub fn create_example_network_manager() -> NetworkManager {
    let pipeline = examples::create_simple_rx_pipeline();
    NetworkManager::with_pipeline(pipeline)
}
