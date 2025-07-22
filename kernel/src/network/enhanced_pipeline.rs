//! Enhanced pipeline implementation with O(1) routing
//!
//! This module implements enhanced pipeline infrastructure with:
//! - Single handler per stage design
//! - HashMap-based O(1) routing
//! - Tx/Rx separated pipeline processing
//! - Protocol-specific builder patterns
//! - Unified handler traits

use hashbrown::HashMap;
use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::{ReceiveHandler, TransmitHandler, NextAction},
};

// ===== Enhanced Pipeline Structures =====

/// A pipeline stage with exactly one handler per direction (1ステージ1ハンドラー設計)
///
/// Unlike the original FlexibleStage which supports multiple processors per stage,
/// this new design has exactly one receive handler and one transmit handler per stage.
/// Each handler is responsible for its own routing decisions using NextStageMatcher.
pub struct FlexibleStage {
    /// Unique identifier for this stage
    pub stage_id: String,
    /// Single receive handler for this stage (optional)
    pub rx_handler: Option<Box<dyn ReceiveHandler>>,
    /// Single transmit handler for this stage (optional)
    pub tx_handler: Option<Box<dyn TransmitHandler>>,
}

impl FlexibleStage {
    /// Create a new stage with the given ID
    pub fn new(stage_id: String) -> Self {
        Self {
            stage_id,
            rx_handler: None,
            tx_handler: None,
        }
    }
    
    /// Set the receive handler for this stage
    pub fn set_rx_handler(&mut self, handler: Box<dyn ReceiveHandler>) {
        self.rx_handler = Some(handler);
    }
    
    /// Set the transmit handler for this stage
    pub fn set_tx_handler(&mut self, handler: Box<dyn TransmitHandler>) {
        self.tx_handler = Some(handler);
    }
    
    /// Check if this stage has a receive handler
    pub fn has_rx_handler(&self) -> bool {
        self.rx_handler.is_some()
    }
    
    /// Check if this stage has a transmit handler
    pub fn has_tx_handler(&self) -> bool {
        self.tx_handler.is_some()
    }
}

/// O(1) high-performance pipeline with HashMap-based stage routing
///
/// This pipeline implements the new Phase 1 design where:
/// - Each stage has exactly one handler per direction
/// - Processing follows NextAction directives from handlers
/// - O(1) stage lookup using HashMap
/// - Complete Tx/Rx separation
pub struct FlexiblePipeline {
    /// Map of stage ID to stage implementation (O(1) lookup)
    stages: HashMap<String, FlexibleStage>,
    /// Default entry stage for receive processing
    default_rx_entry: Option<String>,
    /// Default entry stage for transmit processing
    default_tx_entry: Option<String>,
}

impl FlexiblePipeline {
    /// Create a new empty pipeline
    pub fn new() -> Self {
        Self {
            stages: HashMap::new(),
            default_rx_entry: None,
            default_tx_entry: None,
        }
    }
    
    /// Add a stage to the pipeline
    pub fn add_stage(&mut self, stage: FlexibleStage) -> Result<(), NetworkError> {
        if self.stages.contains_key(&stage.stage_id) {
            return Err(NetworkError::invalid_stage_config(
                &alloc::format!("Stage '{}' already exists", stage.stage_id)
            ));
        }
        
        let stage_id = stage.stage_id.clone();
        self.stages.insert(stage_id, stage);
        Ok(())
    }
    
    /// Remove a stage from the pipeline
    pub fn remove_stage(&mut self, stage_id: &str) -> Option<FlexibleStage> {
        self.stages.remove(stage_id)
    }
    
    /// Get a reference to a stage
    pub fn get_stage(&self, stage_id: &str) -> Option<&FlexibleStage> {
        self.stages.get(stage_id)
    }
    
    /// Set the default receive entry stage
    pub fn set_default_rx_entry(&mut self, stage_id: &str) -> Result<(), NetworkError> {
        if !self.stages.contains_key(stage_id) {
            return Err(NetworkError::stage_not_found(stage_id));
        }
        
        let stage = &self.stages[stage_id];
        if !stage.has_rx_handler() {
            return Err(NetworkError::no_rx_handler(stage_id));
        }
        
        self.default_rx_entry = Some(String::from(stage_id));
        Ok(())
    }
    
    /// Set the default transmit entry stage
    pub fn set_default_tx_entry(&mut self, stage_id: &str) -> Result<(), NetworkError> {
        if !self.stages.contains_key(stage_id) {
            return Err(NetworkError::stage_not_found(stage_id));
        }
        
        let stage = &self.stages[stage_id];
        if !stage.has_tx_handler() {
            return Err(NetworkError::no_tx_handler(stage_id));
        }
        
        self.default_tx_entry = Some(String::from(stage_id));
        Ok(())
    }
    
    /// Process a received packet through the pipeline
    ///
    /// Entry point for receive packet processing. Follows the NextAction chain
    /// from handlers until completion, drop, or termination.
    ///
    /// # Arguments
    /// * `packet` - The packet to process
    /// * `entry_stage` - Optional entry stage; uses default if None
    ///
    /// # Returns
    /// * `Ok(NetworkPacket)` - Processed packet
    /// * `Err(NetworkError)` - Processing failed
    pub fn process_receive(&self, mut packet: NetworkPacket, entry_stage: Option<&str>) -> Result<NetworkPacket, NetworkError> {
        let mut current_stage = match entry_stage {
            Some(stage) => stage.to_string(),
            None => self.default_rx_entry.as_ref()
                .ok_or_else(|| NetworkError::invalid_operation("no rx entry stage specified"))?
                .clone(),
        };
        
        // Main processing loop - follows NextAction chain
        loop {
            let stage = self.stages.get(&current_stage)
                .ok_or_else(|| NetworkError::stage_not_found(&current_stage))?;
            
            let handler = stage.rx_handler.as_ref()
                .ok_or_else(|| NetworkError::no_rx_handler(&current_stage))?;
            
            match handler.handle(&mut packet)? {
                NextAction::Jump(next_stage) => {
                    current_stage = next_stage;
                    continue;
                }
                NextAction::Complete => {
                    return Ok(packet);
                }
                NextAction::Drop(_reason) => {
                    // In a real implementation, might want to log the drop reason
                    return Ok(packet); // Return packet even if logically dropped
                }
                NextAction::Terminate => {
                    // Termination (e.g., waiting for fragments)
                    return Ok(packet);
                }
            }
        }
    }
    
    /// Process a packet for transmission through the pipeline
    ///
    /// Entry point for transmit packet processing. Follows the NextAction chain
    /// from handlers until completion, drop, or termination.
    ///
    /// # Arguments
    /// * `packet` - The packet to process
    /// * `entry_stage` - Optional entry stage; uses default if None
    ///
    /// # Returns
    /// * `Ok(NetworkPacket)` - Processed packet ready for transmission
    /// * `Err(NetworkError)` - Processing failed
    pub fn process_transmit(&self, mut packet: NetworkPacket, entry_stage: Option<&str>) -> Result<NetworkPacket, NetworkError> {
        let mut current_stage = match entry_stage {
            Some(stage) => stage.to_string(),
            None => self.default_tx_entry.as_ref()
                .ok_or_else(|| NetworkError::invalid_operation("no tx entry stage specified"))?
                .clone(),
        };
        
        // Main processing loop - follows NextAction chain
        loop {
            let stage = self.stages.get(&current_stage)
                .ok_or_else(|| NetworkError::stage_not_found(&current_stage))?;
            
            let handler = stage.tx_handler.as_ref()
                .ok_or_else(|| NetworkError::no_tx_handler(&current_stage))?;
            
            match handler.handle(&mut packet)? {
                NextAction::Jump(next_stage) => {
                    current_stage = next_stage;
                    continue;
                }
                NextAction::Complete => {
                    return Ok(packet);
                }
                NextAction::Drop(_reason) => {
                    // In a real implementation, might want to log the drop reason
                    return Ok(packet); // Return packet even if logically dropped
                }
                NextAction::Terminate => {
                    // Termination (e.g., flow control)
                    return Ok(packet);
                }
            }
        }
    }
    
    /// Get the number of stages in the pipeline
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }
    
    /// Check if a stage exists
    pub fn has_stage(&self, stage_id: &str) -> bool {
        self.stages.contains_key(stage_id)
    }
    
    /// Get all stage IDs
    pub fn stage_ids(&self) -> Vec<&str> {
        self.stages.keys().map(|s| s.as_str()).collect()
    }
    
    /// Get the default receive entry stage
    pub fn get_default_rx_entry(&self) -> Option<&str> {
        self.default_rx_entry.as_deref()
    }
    
    /// Get the default transmit entry stage
    pub fn get_default_tx_entry(&self) -> Option<&str> {
        self.default_tx_entry.as_deref()
    }
}

impl Default for FlexiblePipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ===== Builder Pattern Implementation =====

/// Builder for constructing FlexiblePipeline instances
///
/// Provides a fluent API for building pipelines with method chaining.
pub struct FlexiblePipelineBuilder {
    stages: Vec<FlexibleStage>,
    default_rx_entry: Option<String>,
    default_tx_entry: Option<String>,
}

impl FlexiblePipelineBuilder {
    /// Create a new pipeline builder
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            default_rx_entry: None,
            default_tx_entry: None,
        }
    }
    
    /// Add a stage to the pipeline being built
    pub fn add_stage(mut self, stage: FlexibleStage) -> Self {
        self.stages.push(stage);
        self
    }
    
    /// Set the default receive entry stage
    pub fn set_default_rx_entry(mut self, stage_name: &str) -> Self {
        self.default_rx_entry = Some(String::from(stage_name));
        self
    }
    
    /// Set the default transmit entry stage
    pub fn set_default_tx_entry(mut self, stage_name: &str) -> Self {
        self.default_tx_entry = Some(String::from(stage_name));
        self
    }
    
    /// Build the final pipeline
    pub fn build(self) -> Result<FlexiblePipeline, NetworkError> {
        let mut pipeline = FlexiblePipeline::new();
        
        // Add all stages
        for stage in self.stages {
            pipeline.add_stage(stage)?;
        }
        
        // Set default entry stages if specified
        if let Some(rx_entry) = &self.default_rx_entry {
            pipeline.set_default_rx_entry(rx_entry)?;
        }
        
        if let Some(tx_entry) = &self.default_tx_entry {
            pipeline.set_default_tx_entry(tx_entry)?;
        }
        
        Ok(pipeline)
    }
}

impl FlexiblePipeline {
    /// Create a new pipeline builder
    pub fn builder() -> FlexiblePipelineBuilder {
        FlexiblePipelineBuilder::new()
    }
}

impl Default for FlexiblePipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, vec};

    // Mock implementations for testing

    struct MockReceiveHandler {
        next_stage: Option<String>,
        header_name: String,
    }

    impl MockReceiveHandler {
        fn new(header_name: &str, next_stage: Option<&str>) -> Self {
            Self {
                next_stage: next_stage.map(|s| s.to_string()),
                header_name: header_name.to_string(),
            }
        }
    }

    impl ReceiveHandler for MockReceiveHandler {
        fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
            // Add a mock header
            packet.add_header(&self.header_name, vec![0x01, 0x02]);
            
            // Simulate consuming some payload
            let payload = packet.payload();
            if payload.len() > 2 {
                packet.set_payload(payload[2..].to_vec());
            }
            
            match &self.next_stage {
                Some(stage) => Ok(NextAction::jump_to(stage)),
                None => Ok(NextAction::Complete),
            }
        }
    }

    struct MockTransmitHandler {
        next_stage: Option<String>,
        header_bytes: Vec<u8>,
    }

    impl MockTransmitHandler {
        fn new(header_bytes: Vec<u8>, next_stage: Option<&str>) -> Self {
            Self {
                next_stage: next_stage.map(|s| s.to_string()),
                header_bytes,
            }
        }
    }

    impl TransmitHandler for MockTransmitHandler {
        fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
            // Prepend mock header to payload
            let mut new_payload = self.header_bytes.clone();
            new_payload.extend_from_slice(packet.payload());
            packet.set_payload(new_payload);
            
            match &self.next_stage {
                Some(stage) => Ok(NextAction::jump_to(stage)),
                None => Ok(NextAction::Complete),
            }
        }
    }

    #[test_case]
    fn test_flexible_stage_creation() {
        let stage = FlexibleStage::new("test".to_string());
        assert_eq!(stage.stage_id, "test");
        assert!(!stage.has_rx_handler());
        assert!(!stage.has_tx_handler());
    }

    #[test_case]
    fn test_flexible_stage_handlers() {
        let mut stage = FlexibleStage::new("test".to_string());
        
        // Add rx handler
        stage.set_rx_handler(Box::new(MockReceiveHandler::new("test", None)));
        assert!(stage.has_rx_handler());
        assert!(!stage.has_tx_handler());
        
        // Add tx handler
        stage.set_tx_handler(Box::new(MockTransmitHandler::new(vec![0xAA], None)));
        assert!(stage.has_rx_handler());
        assert!(stage.has_tx_handler());
    }

    #[test_case]
    fn test_flexible_pipeline_basic() {
        let mut pipeline = FlexiblePipeline::new();
        assert_eq!(pipeline.stage_count(), 0);
        
        let stage = FlexibleStage::new("test".to_string());
        pipeline.add_stage(stage).unwrap();
        
        assert_eq!(pipeline.stage_count(), 1);
        assert!(pipeline.has_stage("test"));
        assert!(!pipeline.has_stage("nonexistent"));
    }

    #[test_case]
    fn test_pipeline_builder() {
        let mut stage1 = FlexibleStage::new("stage1".to_string());
        stage1.set_rx_handler(Box::new(MockReceiveHandler::new("stage1", Some("stage2"))));
        
        let mut stage2 = FlexibleStage::new("stage2".to_string());
        stage2.set_rx_handler(Box::new(MockReceiveHandler::new("stage2", None)));
        
        let pipeline = FlexiblePipeline::builder()
            .add_stage(stage1)
            .add_stage(stage2)
            .set_default_rx_entry("stage1")
            .build()
            .unwrap();
        
        assert_eq!(pipeline.stage_count(), 2);
        assert_eq!(pipeline.get_default_rx_entry(), Some("stage1"));
    }

    #[test_case]
    fn test_receive_processing() {
        let mut stage1 = FlexibleStage::new("stage1".to_string());
        stage1.set_rx_handler(Box::new(MockReceiveHandler::new("stage1", Some("stage2"))));
        
        let mut stage2 = FlexibleStage::new("stage2".to_string());
        stage2.set_rx_handler(Box::new(MockReceiveHandler::new("stage2", None)));
        
        let pipeline = FlexiblePipeline::builder()
            .add_stage(stage1)
            .add_stage(stage2)
            .set_default_rx_entry("stage1")
            .build()
            .unwrap();
        
        let packet = NetworkPacket::new(
            vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
            "test".to_string()
        );
        
        let result = pipeline.process_receive(packet, None);
        assert!(result.is_ok());
        
        let processed_packet = result.unwrap();
        assert_eq!(processed_packet.get_header("stage1"), Some(&[0x01, 0x02][..]));
        assert_eq!(processed_packet.get_header("stage2"), Some(&[0x01, 0x02][..]));
        assert_eq!(processed_packet.payload(), &[0x03, 0x04]); // Both stages consumed 2 bytes each
    }

    #[test_case]
    fn test_transmit_processing() {
        let mut stage1 = FlexibleStage::new("stage1".to_string());
        stage1.set_tx_handler(Box::new(MockTransmitHandler::new(vec![0xAA, 0xBB], Some("stage2"))));
        
        let mut stage2 = FlexibleStage::new("stage2".to_string());
        stage2.set_tx_handler(Box::new(MockTransmitHandler::new(vec![0xCC, 0xDD], None)));
        
        let pipeline = FlexiblePipeline::builder()
            .add_stage(stage1)
            .add_stage(stage2)
            .set_default_tx_entry("stage1")
            .build()
            .unwrap();
        
        let packet = NetworkPacket::new(
            vec![0x01, 0x02],
            "test".to_string()
        );
        
        let result = pipeline.process_transmit(packet, None);
        assert!(result.is_ok());
        
        let processed_packet = result.unwrap();
        // Headers should be prepended in order: stage2, stage1, original payload
        assert_eq!(processed_packet.payload(), &[0xCC, 0xDD, 0xAA, 0xBB, 0x01, 0x02]);
    }

    #[test_case]
    fn test_error_cases() {
        let pipeline = FlexiblePipeline::new();
        let packet = NetworkPacket::new(vec![0x01], "test".to_string());
        
        // No default rx entry
        let result = pipeline.process_receive(packet, None);
        assert!(result.is_err());
        
        // Nonexistent stage
        let packet = NetworkPacket::new(vec![0x01], "test".to_string());
        let result = pipeline.process_receive(packet, Some("nonexistent"));
        assert!(result.is_err());
    }

    #[test_case]
    fn test_stage_without_handler() {
        let mut pipeline = FlexiblePipeline::new();
        let stage = FlexibleStage::new("test".to_string());
        pipeline.add_stage(stage).unwrap();
        
        // Try to set stage without rx handler as default rx entry
        let result = pipeline.set_default_rx_entry("test");
        assert!(result.is_err());
        match result {
            Err(NetworkError::NoRxHandler(stage)) => {
                assert_eq!(stage, "test");
            }
            _ => panic!("Expected NoRxHandler error"),
        }
    }
}