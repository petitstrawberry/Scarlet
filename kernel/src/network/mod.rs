//! Network protocol stack
//!
//! This module implements the network protocol stack for Scarlet kernel,
//! featuring a flexible pipeline architecture with Tx/Rx separation.
//!
//! ## Architecture Overview
//!
//! The network stack supports two pipeline architectures:
//!
//! ### Original Multi-Processor Pipeline (Backward Compatible)
//! - **NetworkManager**: Orchestrates packet processing through pipelines
//! - **FlexiblePipeline**: Manages collections of processing stages
//! - **FlexibleStage**: Groups processors for specific protocol layers
//! - **RxStageProcessor/TxStageProcessor**: Tx/Rx separated packet processors
//! - **RxStageHandler/TxStageBuilder**: Protocol-specific packet handlers
//! - **ProcessorCondition**: Determines packet routing through stages
//!
//! ### Phase 1: 1ステージ1ハンドラー設計 (New O(1) Pipeline)
//! - **phase1::FlexiblePipeline**: O(1) HashMap-based stage routing
//! - **phase1::FlexibleStage**: Exactly one handler per direction per stage
//! - **ReceiveHandler/TransmitHandler**: Unified handlers with internal routing
//! - **NextStageMatcher**: Generic trait for O(1) protocol routing
//! - **matchers**: HashMap-based matchers (EtherType, IP protocol, ports)
//! - **protocols**: Protocol-specific handlers and builders
//!
//! ## Tx/Rx Separation
//!
//! Both architectures separate receive and transmit paths:
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

// Phase 1: New O(1) pipeline infrastructure
pub mod phase1;
pub mod matchers;
pub mod protocols;
pub mod integration;

// Examples and testing
pub mod examples;

// Protocol implementations (Phase 2+)
// pub mod protocol;

// Re-export key types for convenience

// Original infrastructure (backward compatible)
pub use error::NetworkError;
pub use packet::{NetworkPacket, Instant};
pub use traits::{RxStageHandler, TxStageBuilder, ProcessorCondition, NextAction};
pub use pipeline::{FlexiblePipeline, FlexibleStage, RxStageProcessor, TxStageProcessor};
pub use network_manager::{NetworkManager, NetworkProcessingStats};

// Phase 1: New unified handler traits
pub use traits::{ReceiveHandler, TransmitHandler, NextStageMatcher};

// Phase 1: New pipeline infrastructure
pub use phase1::{
    FlexiblePipeline as Phase1Pipeline,
    FlexibleStage as Phase1Stage,
    FlexiblePipelineBuilder,
};

// Phase 1: Protocol routing matchers
pub use matchers::{
    EtherTypeToStage, IpProtocolToStage, PortRangeToStage, PortRange,
    EtherType, IpProtocol,
};

// Phase 1: Protocol handlers and builders
pub use protocols::{
    EthernetRxHandler, EthernetTxHandler, IPv4RxHandler, IPv4TxHandler,
    EthernetStage, EthernetStageBuilder, IPv4Stage, IPv4StageBuilder,
};

/// Create a basic example network manager for testing (original architecture)
pub fn create_example_network_manager() -> NetworkManager {
    let pipeline = examples::create_simple_rx_pipeline();
    NetworkManager::with_pipeline(pipeline)
}

/// Create a Phase 1 example pipeline for demonstration
pub fn create_phase1_example_pipeline() -> Result<phase1::FlexiblePipeline, NetworkError> {
    integration::build_receive_pipeline()
}

/// Run Phase 1 integration tests to verify functionality
pub fn run_phase1_integration_tests() -> Result<(), NetworkError> {
    integration::run_integration_tests()
}
