//! NetworkManager implementation
//!
//! Global network manager that provides a unified interface for packet processing
//! through the FlexiblePipeline. Handles both incoming and outgoing packets.
//!
//! ## Ownership Model
//! 
//! The NetworkManager follows a strict ownership model where NetworkPacket
//! instances are consumed during processing and are NOT returned to external
//! callers. This design ensures:
//! 
//! - Clear ownership semantics: packets are either consumed by handlers or dropped
//! - No accidental packet duplication or memory leaks
//! - Simplified API surface with clear intent
//! 
//! External components should only expect PipelineResult which indicates
//! what action was taken on the packet, not the packet itself.

use spin::{Mutex, Once};

use crate::network::packet::NetworkPacket;
use crate::network::pipeline::{FlexiblePipeline, PipelineResult, StageIdentifier};
use crate::network::error::NetworkError;
use crate::network::protocols::{EthernetStage, EtherType, IPv4Stage, IpProtocol, ArpStage};

/// Global network manager instance
static NETWORK_MANAGER: Once<NetworkManager> = Once::new();

/// Network manager responsible for packet processing coordination
pub struct NetworkManager {
    /// Unified pipeline for processing both incoming and outgoing packets
    pipeline: Option<FlexiblePipeline>,
    /// Statistics
    stats: Mutex<NetworkStats>,
}

/// Network manager statistics
#[derive(Debug, Clone, Default)]
pub struct NetworkStats {
    /// Total packets processed
    pub packets_processed: u64,
    /// Processing errors
    pub errors: u64,
    /// Packets dropped
    pub dropped_packets: u64,
}

impl NetworkManager {
    /// Create a new NetworkManager
    pub fn new() -> Self {
        Self {
            pipeline: None,
            stats: Mutex::new(NetworkStats::default()),
        }
    }

    /// Get the global network manager instance
    pub fn get_manager() -> &'static NetworkManager {
        NETWORK_MANAGER.call_once(|| {
            let mut manager = NetworkManager::new();
            
            // Initialize default pipeline
            if let Ok(pipeline) = Self::create_default_pipeline() {
                manager.pipeline = Some(pipeline);
            }
            
            manager
        })
    }

    /// Process packet through the unified pipeline
    /// 
    /// This is the main entry point for all packet processing.
    /// 
    /// ## Ownership Model
    /// The NetworkPacket is consumed by this method and processed through
    /// the pipeline. The packet is NOT returned to the caller - it is either:
    /// - Consumed by a handler in the pipeline
    /// - Dropped due to processing errors
    /// - Forwarded to another component within the network stack
    /// 
    /// Only the PipelineResult is returned to indicate what action was taken.
    pub fn process(&self, packet: NetworkPacket, entry_stage: Option<&str>) -> Result<PipelineResult, NetworkError> {
        // Process through unified pipeline - packet ownership is transferred to pipeline
        if let Some(ref pipeline) = self.pipeline {
            match pipeline.process(packet, entry_stage) {
                Ok((_processed_packet, result)) => {
                    // Note: _processed_packet is intentionally dropped here
                    // NetworkPacket should not be returned outside NetworkManager
                    let mut stats = self.stats.lock();
                    stats.packets_processed += 1;
                    Ok(result)
                },
                Err(err) => {
                    let mut stats = self.stats.lock();
                    stats.errors += 1;
                    Err(err)
                }
            }
        } else {
            let mut stats = self.stats.lock();
            stats.errors += 1;
            Err(NetworkError::invalid_operation("no pipeline configured"))
        }
    }

    /// Type-safe processing helper - infers entry stage from type
    pub fn process_at<T>(&self, packet: NetworkPacket) -> Result<PipelineResult, NetworkError>
    where 
        T: StageIdentifier,
    {
        self.process(packet, Some(T::stage_id()))
    }

    /// Get network statistics
    pub fn get_stats(&self) -> NetworkStats {
        self.stats.lock().clone()
    }

    /// Reset network statistics
    pub fn reset_stats(&self) {
        *self.stats.lock() = NetworkStats::default();
    }

    /// Create default pipeline with basic protocol support
    fn create_default_pipeline() -> Result<FlexiblePipeline, NetworkError> {
        FlexiblePipeline::builder()
            .add_stage(
                EthernetStage::builder()
                    .add_ethertype_route(EtherType::IPv4 as u16, "ipv4")
                    .add_ethertype_route(EtherType::ARP as u16, "arp")
                    .build()
            )
            .add_stage(
                IPv4Stage::builder()
                    .add_protocol_route(IpProtocol::TCP as u8, "tcp")
                    .add_protocol_route(IpProtocol::UDP as u8, "udp")
                    .add_protocol_route(IpProtocol::ICMP as u8, "icmp")
                    .build()
            )
            .add_stage(ArpStage::builder().build())
            // TODO: Add TCP, UDP, ICMP stages
            .set_default_rx_entry("ethernet")
            .set_default_tx_entry("tcp") // Default for application-generated packets
            .build()
    }
}

/// Global process function for packet processing
/// 
/// This function consumes the NetworkPacket and only returns the processing result.
/// The packet itself is NOT returned to maintain the ownership model where
/// NetworkPacket does not escape NetworkManager.
pub fn process_packet(packet: NetworkPacket, entry_stage: Option<&str>) -> Result<PipelineResult, NetworkError> {
    NetworkManager::get_manager().process(packet, entry_stage)
}

/// Type-safe global process function
/// 
/// Infers the entry stage from the type parameter and processes the packet.
/// The packet is consumed and only the processing result is returned.
pub fn process_packet_at<T>(packet: NetworkPacket) -> Result<PipelineResult, NetworkError>
where
    T: StageIdentifier,
{
    NetworkManager::get_manager().process_at::<T>(packet)
}
