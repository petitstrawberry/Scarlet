use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::network::traits::{ReceiveHandler, TransmitHandler, NextAction};
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

/// Flexible pipeline stage (Tx/Rx separated)
#[derive(Debug)]
pub struct FlexibleStage {
    pub stage_id: String,
    pub rx_handler: Option<Box<dyn ReceiveHandler>>,
    pub tx_handler: Option<Box<dyn TransmitHandler>>,
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

/// Flexible pipeline (Tx/Rx completely separated)
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

    /// Receive processing: start from specified entry
    pub fn process_receive(&self, mut packet: NetworkPacket, entry_stage: Option<&str>) -> Result<NetworkPacket, NetworkError> {
        let mut current_stage = String::from(entry_stage.or(self.default_rx_entry.as_deref())
            .ok_or_else(|| NetworkError::invalid_operation("no rx entry stage specified"))?);
        
        loop {
            let stage = self.stages.get(&current_stage)
                .ok_or_else(|| NetworkError::stage_not_found(&current_stage))?;
            
            let handler = stage.rx_handler.as_ref()
                .ok_or_else(|| NetworkError::no_rx_handler(&current_stage))?;
            
            match handler.handle(&mut packet)? {
                NextAction::JumpTo(next_stage) => {
                    current_stage = next_stage;
                }
                NextAction::Complete => {
                    return Ok(packet);
                }
            }
        }
    }
    
    /// Transmit processing: start from specified entry
    pub fn process_transmit(&self, mut packet: NetworkPacket, entry_stage: Option<&str>) -> Result<NetworkPacket, NetworkError> {
        let mut current_stage = String::from(entry_stage.or(self.default_tx_entry.as_deref())
            .ok_or_else(|| NetworkError::invalid_operation("no tx entry stage specified"))?);
        
        loop {
            let stage = self.stages.get(&current_stage)
                .ok_or_else(|| NetworkError::stage_not_found(&current_stage))?;
            
            let handler = stage.tx_handler.as_ref()
                .ok_or_else(|| NetworkError::no_tx_handler(&current_stage))?;
            
            match handler.handle(&mut packet)? {
                NextAction::JumpTo(next_stage) => {
                    current_stage = next_stage;
                }
                NextAction::Complete => {
                    return Ok(packet);
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
