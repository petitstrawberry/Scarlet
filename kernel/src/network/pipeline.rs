use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::boxed::Box;
use crate::network::traits::{PacketHandler, NextAction};
use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;

/// Stage identifier trait for type-safe stage management
/// 
/// This trait allows modules to define their own stages while maintaining
/// type safety and avoiding string-based errors.
pub trait StageIdentifier {
    /// Get the stage identifier
    fn stage_id() -> &'static str;
}

/// Helper macro to implement StageIdentifier
#[macro_export]
macro_rules! define_stage {
    ($stage_type:ident, $stage:literal) => {
        impl $crate::network::pipeline::StageIdentifier for $stage_type {
            fn stage_id() -> &'static str {
                $stage
            }
        }
    };
}

/// Flexible pipeline stage
/// 
/// Each stage maintains separate handlers for receive (rx) and transmit (tx) directions.
/// The pipeline itself is unified, but handlers are direction-specific.
#[derive(Debug)]
pub struct FlexibleStage {
    pub stage_id: String,
    /// Handler for incoming packets (device -> application)
    pub rx_handler: Option<Box<dyn PacketHandler>>,
    /// Handler for outgoing packets (application -> device)  
    pub tx_handler: Option<Box<dyn PacketHandler>>,
}

impl FlexibleStage {
    /// Create a new FlexibleStage
    pub fn new(stage_id: String) -> Self {
        Self {
            stage_id,
            rx_handler: None,
            tx_handler: None,
        }
    }
    
    /// Create a new FlexibleStage with type-safe identifier
    pub fn new_typed<T: StageIdentifier>() -> Self {
        Self {
            stage_id: T::stage_id().to_string(),
            rx_handler: None,
            tx_handler: None,
        }
    }
}

/// Pipeline processing result
#[derive(Debug, Clone)]
pub enum PipelineResult {
    /// Packet should be sent to specified device
    ToDevice(String),
    /// Packet should be delivered to application layer
    ToApplication,
    /// Packet was dropped during processing
    Dropped,
}

/// Flexible pipeline (unified)
#[derive(Debug)]
pub struct FlexiblePipeline {
    stages: BTreeMap<String, FlexibleStage>,
    default_rx_entry: Option<String>,
    default_tx_entry: Option<String>,
}

impl FlexiblePipeline {
    /// Create a pipeline builder
    pub fn builder() -> FlexiblePipelineBuilder {
        FlexiblePipelineBuilder::new()
    }

    /// Process packet through pipeline (unified processing)
    pub fn process(&self, mut packet: NetworkPacket, entry_stage: Option<&str>) -> Result<(NetworkPacket, PipelineResult), NetworkError> {
        // Determine default entry stage based on packet direction
        let default_entry = match packet.direction() {
            crate::network::packet::PacketDirection::Incoming => self.default_rx_entry.as_deref(),
            crate::network::packet::PacketDirection::Outgoing => self.default_tx_entry.as_deref(),
        };
        
        let mut current_stage = String::from(entry_stage.or(default_entry)
            .ok_or_else(|| NetworkError::invalid_operation("no entry stage specified"))?);
        
        loop {
            let stage = self.stages.get(&current_stage)
                .ok_or_else(|| NetworkError::stage_not_found(&current_stage))?;
            
            let action = match packet.direction() {
                crate::network::packet::PacketDirection::Incoming => {
                    let handler = stage.rx_handler.as_ref()
                        .ok_or_else(|| NetworkError::no_rx_handler(&current_stage))?;
                    handler.handle(&mut packet)?
                }
                crate::network::packet::PacketDirection::Outgoing => {
                    let handler = stage.tx_handler.as_ref()
                        .ok_or_else(|| NetworkError::no_tx_handler(&current_stage))?;
                    handler.handle(&mut packet)?
                }
            };
            
            match action {
                NextAction::JumpTo(next_stage) => {
                    current_stage = next_stage;
                }
                NextAction::CompleteToDevice(device_name) => {
                    return Ok((packet, PipelineResult::ToDevice(device_name)));
                }
                NextAction::CompleteToApplication => {
                    return Ok((packet, PipelineResult::ToApplication));
                }
                NextAction::Drop => {
                    return Ok((packet, PipelineResult::Dropped));
                }
                NextAction::ChangeDirection { stage, new_packet } => {
                    // Change packet direction and continue processing
                    if let Some(new_pkt) = new_packet {
                        packet = new_pkt;
                    }
                    // Toggle packet direction
                    packet.set_direction(packet.direction().opposite());
                    current_stage = stage;
                }
                NextAction::SpawnInOppositeDirection { stage, new_packet, continue_to: _ } => {
                    // Spawn new packet in opposite direction (would need async processing)
                    // For now, just process the new packet and ignore the original
                    // TODO: Implement proper async spawning
                    packet = new_packet;
                    packet.set_direction(packet.direction().opposite());
                    current_stage = stage;
                    
                    // Note: This is a simplified implementation
                    // In a full implementation, we'd need to spawn a separate task
                    // to process the original packet according to continue_to
                }
            }
        }
    }

    /// Check if stage exists
    pub fn has_stage(&self, stage_id: &str) -> bool {
        self.stages.contains_key(stage_id)
    }
    
    /// Check if stage exists (type-safe)
    pub fn has_stage_typed<T: StageIdentifier>(&self) -> bool {
        self.stages.contains_key(T::stage_id())
    }

    /// Get all stage IDs
    pub fn stage_ids(&self) -> Vec<String> {
        self.stages.keys().cloned().collect()
    }
}

/// FlexiblePipeline builder (generic)
pub struct FlexiblePipelineBuilder {
    stages: Vec<FlexibleStage>,
    default_rx_entry: Option<String>,
    default_tx_entry: Option<String>,
}

impl FlexiblePipelineBuilder {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            default_rx_entry: None,
            default_tx_entry: None,
        }
    }
    
    pub fn add_stage(mut self, stage: FlexibleStage) -> Self {
        self.stages.push(stage);
        self
    }
    
    pub fn set_default_rx_entry(mut self, stage_name: &str) -> Self {
        self.default_rx_entry = Some(String::from(stage_name));
        self
    }
    
    /// Set default rx entry (type-safe)
    pub fn set_default_rx_entry_typed<T: StageIdentifier>(mut self) -> Self {
        self.default_rx_entry = Some(T::stage_id().to_string());
        self
    }
    
    pub fn set_default_tx_entry(mut self, stage_name: &str) -> Self {
        self.default_tx_entry = Some(String::from(stage_name));
        self
    }
    
    /// Set default tx entry (type-safe)
    pub fn set_default_tx_entry_typed<T: StageIdentifier>(mut self) -> Self {
        self.default_tx_entry = Some(T::stage_id().to_string());
        self
    }
    
    pub fn build(self) -> Result<FlexiblePipeline, NetworkError> {
        let mut stage_map = BTreeMap::new();
        for stage in self.stages {
            stage_map.insert(stage.stage_id.clone(), stage);
        }
        
        Ok(FlexiblePipeline {
            stages: stage_map,
            default_rx_entry: self.default_rx_entry,
            default_tx_entry: self.default_tx_entry,
        })
    }
}
