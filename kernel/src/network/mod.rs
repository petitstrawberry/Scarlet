//! Network protocol stack
//!
//! This module implements the network protocol stack for Scarlet kernel,
//! featuring a flexible pipeline architecture with Tx/Rx separation.

// Core infrastructure
pub mod error;
pub mod packet;
pub mod traits;
pub mod pipeline;
pub mod network_manager;

// Enhanced infrastructure 
pub mod enhanced_pipeline;
pub mod matchers;
pub mod protocols;
pub mod integration;

// Examples and testing
pub mod examples;

// Re-export key types for convenience

// Original infrastructure (backward compatible)
pub use error::NetworkError;
pub use packet::{NetworkPacket, Instant};
pub use traits::{RxStageHandler, TxStageBuilder, ProcessorCondition, NextAction};
pub use pipeline::{FlexiblePipeline, FlexibleStage, RxStageProcessor, TxStageProcessor};
pub use network_manager::{NetworkManager, NetworkProcessingStats};

// Enhanced handler traits
pub use traits::{ReceiveHandler, TransmitHandler, NextStageMatcher};

// Enhanced pipeline infrastructure
pub use enhanced_pipeline::{
    FlexiblePipeline as EnhancedPipeline,
    FlexibleStage as EnhancedStage,
    FlexiblePipelineBuilder,
};

// Protocol routing matchers
pub use matchers::{
    EtherTypeToStage, IpProtocolToStage, PortRangeToStage, PortRange,
    EtherType, IpProtocol,
};

// Protocol handlers and builders
pub use protocols::{
    EthernetRxHandler, EthernetTxHandler, IPv4RxHandler, IPv4TxHandler,
    EthernetStage, EthernetStageBuilder, IPv4Stage, IPv4StageBuilder,
};

/// Create a basic example network manager for testing (original architecture)
pub fn create_example_network_manager() -> NetworkManager {
    let pipeline = examples::create_simple_rx_pipeline();
    NetworkManager::with_pipeline(pipeline)
}

/// Create an enhanced example pipeline for demonstration
pub fn create_enhanced_example_pipeline() -> Result<enhanced_pipeline::FlexiblePipeline, NetworkError> {
    integration::build_receive_pipeline()
}

/// Run integration tests to verify functionality
pub fn run_integration_tests() -> Result<(), NetworkError> {
    integration::run_integration_tests()
}
