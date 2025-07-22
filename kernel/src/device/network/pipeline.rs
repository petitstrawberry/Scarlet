//! Pipeline infrastructure for flexible network packet processing
//!
//! This module implements the core pipeline components that enable flexible,
//! protocol-independent packet processing through staged processors.

use hashbrown::HashMap;
use alloc::{
    string::String,
    vec::Vec,
    boxed::Box,
};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::{StageHandler, ProcessorCondition, NextAction},
};

/// A single processor within a pipeline stage
///
/// Each StageProcessor combines:
/// - A condition that determines if this processor should handle a packet
/// - A handler that performs the actual packet processing
/// - An action that specifies what to do after processing
pub struct StageProcessor {
    /// Condition to check if this processor should handle the packet
    pub condition: Box<dyn ProcessorCondition>,
    /// Handler to process the packet if condition matches
    pub handler: Box<dyn StageHandler>,
    /// Action to take after successful processing
    pub next_action: NextAction,
}

impl StageProcessor {
    /// Create a new stage processor
    ///
    /// # Arguments
    /// * `condition` - Condition checker for this processor
    /// * `handler` - Packet handler for this processor  
    /// * `next_action` - Action to take after processing
    pub fn new(
        condition: Box<dyn ProcessorCondition>,
        handler: Box<dyn StageHandler>,
        next_action: NextAction,
    ) -> Self {
        Self {
            condition,
            handler,
            next_action,
        }
    }

    /// Check if this processor should handle the given packet
    pub fn matches(&self, packet: &NetworkPacket) -> bool {
        self.condition.matches(packet)
    }

    /// Process the packet with this processor's handler
    pub fn process(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        self.handler.handle(packet)?;
        Ok(self.next_action.clone())
    }
}

/// A stage in the processing pipeline
///
/// Each FlexibleStage represents a protocol layer (e.g., ethernet, ipv4, tcp)
/// and contains multiple processors that can handle different packet variants
/// within that layer.
pub struct FlexibleStage {
    /// Unique identifier for this stage
    pub stage_id: String,
    /// List of processors in this stage (tried in order)
    pub processors: Vec<StageProcessor>,
}

impl FlexibleStage {
    /// Create a new flexible stage
    ///
    /// # Arguments
    /// * `stage_id` - Unique identifier for this stage
    pub fn new(stage_id: &str) -> Self {
        Self {
            stage_id: String::from(stage_id),
            processors: Vec::new(),
        }
    }

    /// Add a processor to this stage
    ///
    /// Processors are tried in the order they are added.
    /// The first processor whose condition matches will handle the packet.
    pub fn add_processor(&mut self, processor: StageProcessor) {
        self.processors.push(processor);
    }

    /// Remove all processors from this stage
    pub fn clear_processors(&mut self) {
        self.processors.clear();
    }

    /// Get the number of processors in this stage
    pub fn processor_count(&self) -> usize {
        self.processors.len()
    }

    /// Process a packet through this stage
    ///
    /// Tries processors in order until one matches and processes the packet.
    /// Returns the NextAction from the matching processor.
    ///
    /// # Arguments
    /// * `packet` - The packet to process
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take after processing
    /// * `Err(NetworkError)` - If no processor matches or processing fails
    pub fn process_packet(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        for processor in &self.processors {
            if processor.matches(packet) {
                return processor.process(packet);
            }
        }
        
        Err(NetworkError::no_matching_processor(&self.stage_id))
    }
}

/// The main pipeline that orchestrates packet processing through stages
///
/// FlexiblePipeline manages a collection of stages and provides methods
/// for adding, removing, and organizing stages for packet processing.
pub struct FlexiblePipeline {
    /// Map of stage ID to stage implementation
    stages: HashMap<String, FlexibleStage>,
    /// Default starting stage for packet processing
    default_entry_stage: Option<String>,
}

impl FlexiblePipeline {
    /// Create a new empty pipeline
    pub fn new() -> Self {
        Self {
            stages: HashMap::new(),
            default_entry_stage: None,
        }
    }

    /// Add a stage to the pipeline
    ///
    /// # Arguments
    /// * `stage` - The stage to add
    ///
    /// # Returns
    /// * `Ok(())` if successful
    /// * `Err(NetworkError)` if a stage with the same ID already exists
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
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to remove
    ///
    /// # Returns
    /// * `Some(FlexibleStage)` - The removed stage
    /// * `None` - If no stage with that ID exists
    pub fn remove_stage(&mut self, stage_id: &str) -> Option<FlexibleStage> {
        self.stages.remove(stage_id)
    }

    /// Get a reference to a stage
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to retrieve
    ///
    /// # Returns
    /// * `Some(&FlexibleStage)` - Reference to the stage
    /// * `None` - If no stage with that ID exists
    pub fn get_stage(&self, stage_id: &str) -> Option<&FlexibleStage> {
        self.stages.get(stage_id)
    }

    /// Get a mutable reference to a stage
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to retrieve
    ///
    /// # Returns
    /// * `Some(&mut FlexibleStage)` - Mutable reference to the stage
    /// * `None` - If no stage with that ID exists
    pub fn get_stage_mut(&mut self, stage_id: &str) -> Option<&mut FlexibleStage> {
        self.stages.get_mut(stage_id)
    }

    /// Set the default entry stage for packet processing
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to use as default entry point
    ///
    /// # Returns
    /// * `Ok(())` if successful
    /// * `Err(NetworkError)` if the stage doesn't exist or has no processors
    pub fn set_default_entry_stage(&mut self, stage_id: &str) -> Result<(), NetworkError> {
        if !self.stages.contains_key(stage_id) {
            return Err(NetworkError::stage_not_found(stage_id));
        }
        
        // Check if the stage has at least one processor
        let stage = &self.stages[stage_id];
        if stage.processor_count() == 0 {
            return Err(NetworkError::invalid_stage_config(
                &alloc::format!("Stage '{}' has no processors and cannot be used as default entry", stage_id)
            ));
        }
        
        self.default_entry_stage = Some(String::from(stage_id));
        Ok(())
    }

    /// Get the default entry stage ID
    pub fn get_default_entry_stage(&self) -> Option<&str> {
        self.default_entry_stage.as_deref()
    }

    /// Get all stage IDs in the pipeline
    pub fn stage_ids(&self) -> Vec<&str> {
        self.stages.keys().map(|s| s.as_str()).collect()
    }

    /// Get the total number of stages
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    /// Check if a stage exists
    pub fn has_stage(&self, stage_id: &str) -> bool {
        self.stages.contains_key(stage_id)
    }

    /// Process a packet through a specific stage
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to process with
    /// * `packet` - The packet to process
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take after processing
    /// * `Err(NetworkError)` - If stage not found or processing fails
    pub fn process_packet_in_stage(
        &self, 
        stage_id: &str, 
        packet: &mut NetworkPacket
    ) -> Result<NextAction, NetworkError> {
        let stage = self.stages.get(stage_id)
            .ok_or_else(|| NetworkError::stage_not_found(stage_id))?;
        
        stage.process_packet(packet)
    }

    /// Validate pipeline configuration
    ///
    /// Checks for common configuration errors like circular dependencies.
    /// This is a basic implementation - a full version might do more sophisticated
    /// dependency analysis.
    pub fn validate(&self) -> Result<(), NetworkError> {
        // Basic validation: check that default entry stage exists
        if let Some(entry_stage) = &self.default_entry_stage {
            if !self.stages.contains_key(entry_stage) {
                return Err(NetworkError::invalid_stage_config(
                    &alloc::format!("Default entry stage '{}' does not exist", entry_stage)
                ));
            }
        }

        // Check that all stages have at least one processor
        for (stage_id, stage) in &self.stages {
            if stage.processors.is_empty() {
                return Err(NetworkError::invalid_stage_config(
                    &alloc::format!("Stage '{}' has no processors", stage_id)
                ));
            }
        }

        Ok(())
    }
}

impl Default for FlexiblePipeline {
    fn default() -> Self {
        Self::new()
    }
}

// Manual Debug implementations since trait objects don't implement Debug
impl core::fmt::Debug for StageProcessor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StageProcessor")
            .field("next_action", &self.next_action)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for FlexibleStage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FlexibleStage")
            .field("stage_id", &self.stage_id)
            .field("processor_count", &self.processors.len())
            .finish()
    }
}

impl core::fmt::Debug for FlexiblePipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FlexiblePipeline")
            .field("stage_count", &self.stages.len())
            .field("default_entry_stage", &self.default_entry_stage)
            .field("stages", &self.stages.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // Mock implementations for testing

    struct MockHandler {
        header_name: String,
    }

    impl MockHandler {
        fn new(header_name: &str) -> Self {
            Self { header_name: String::from(header_name) }
        }
    }

    impl StageHandler for MockHandler {
        fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            packet.add_header(&self.header_name, vec![0x01, 0x02]);
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

    #[test_case]
    fn test_stage_processor_creation() {
        let processor = StageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockHandler::new("test")),
            NextAction::Complete,
        );

        let packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        assert!(processor.matches(&packet));
    }

    #[test_case]
    fn test_stage_processor_processing() {
        let processor = StageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockHandler::new("test")),
            NextAction::jump_to("next"),
        );

        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = processor.process(&mut packet);
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::jump_to("next"));
        assert_eq!(packet.get_header("test"), Some(&[0x01, 0x02][..]));
    }

    #[test_case]
    fn test_flexible_stage() {
        let mut stage = FlexibleStage::new("test_stage");
        assert_eq!(stage.stage_id, "test_stage");
        assert_eq!(stage.processor_count(), 0);

        // Add processors
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition { should_match: false }),
            Box::new(MockHandler::new("handler1")),
            NextAction::Complete,
        ));

        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockHandler::new("handler2")),
            NextAction::jump_to("next"),
        ));

        assert_eq!(stage.processor_count(), 2);

        // Test packet processing - should match second processor
        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = stage.process_packet(&mut packet);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::jump_to("next"));
        assert_eq!(packet.get_header("handler2"), Some(&[0x01, 0x02][..]));
        assert!(packet.get_header("handler1").is_none()); // First processor didn't run
    }

    #[test_case]
    fn test_flexible_stage_no_match() {
        let mut stage = FlexibleStage::new("test_stage");
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition { should_match: false }),
            Box::new(MockHandler::new("handler")),
            NextAction::Complete,
        ));

        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = stage.process_packet(&mut packet);

        assert!(result.is_err());
        match result {
            Err(NetworkError::NoMatchingProcessor(stage_id)) => {
                assert_eq!(stage_id, "test_stage");
            }
            _ => panic!("Expected NoMatchingProcessor error"),
        }
    }

    #[test_case]
    fn test_flexible_pipeline_basic() {
        let mut pipeline = FlexiblePipeline::new();
        assert_eq!(pipeline.stage_count(), 0);

        // Add a stage
        let stage = FlexibleStage::new("ethernet");
        pipeline.add_stage(stage).unwrap();

        assert_eq!(pipeline.stage_count(), 1);
        assert!(pipeline.has_stage("ethernet"));
        assert!(!pipeline.has_stage("ipv4"));

        // Test stage retrieval
        assert!(pipeline.get_stage("ethernet").is_some());
        assert!(pipeline.get_stage("ipv4").is_none());

        let stage_ids = pipeline.stage_ids();
        assert_eq!(stage_ids.len(), 1);
        assert!(stage_ids.contains(&"ethernet"));
    }

    #[test_case]
    fn test_flexible_pipeline_duplicate_stage() {
        let mut pipeline = FlexiblePipeline::new();
        
        let stage1 = FlexibleStage::new("ethernet");
        pipeline.add_stage(stage1).unwrap();

        let stage2 = FlexibleStage::new("ethernet");
        let result = pipeline.add_stage(stage2);
        
        assert!(result.is_err());
        match result {
            Err(NetworkError::InvalidStageConfig(msg)) => {
                assert!(msg.contains("ethernet"));
                assert!(msg.contains("already exists"));
            }
            _ => panic!("Expected InvalidStageConfig error"),
        }
    }

    #[test_case]
    fn test_pipeline_default_entry_stage() {
        let mut pipeline = FlexiblePipeline::new();
        assert!(pipeline.get_default_entry_stage().is_none());

        // Try to set non-existent stage as default
        let result = pipeline.set_default_entry_stage("ethernet");
        assert!(result.is_err());

        // Add empty stage and try to set as default - should fail
        let empty_stage = FlexibleStage::new("ethernet");
        pipeline.add_stage(empty_stage).unwrap();
        let result = pipeline.set_default_entry_stage("ethernet");
        assert!(result.is_err()); // Should fail because stage has no processors

        // Remove empty stage
        pipeline.remove_stage("ethernet");

        // Add stage with processor and set as default
        let mut stage = FlexibleStage::new("ethernet");
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockHandler::new("test")),
            NextAction::Complete,
        ));
        pipeline.add_stage(stage).unwrap();
        pipeline.set_default_entry_stage("ethernet").unwrap();

        assert_eq!(pipeline.get_default_entry_stage(), Some("ethernet"));
    }

    #[test_case]
    fn test_pipeline_process_packet_in_stage() {
        let mut pipeline = FlexiblePipeline::new();
        
        // Create and add a stage with a processor
        let mut stage = FlexibleStage::new("test");
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockHandler::new("test_header")),
            NextAction::Complete,
        ));
        pipeline.add_stage(stage).unwrap();

        // Test processing
        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = pipeline.process_packet_in_stage("test", &mut packet);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::Complete);
        assert_eq!(packet.get_header("test_header"), Some(&[0x01, 0x02][..]));

        // Test with non-existent stage
        let result = pipeline.process_packet_in_stage("nonexistent", &mut packet);
        assert!(result.is_err());
        match result {
            Err(NetworkError::StageNotFound(stage)) => {
                assert_eq!(stage, "nonexistent");
            }
            _ => panic!("Expected StageNotFound error"),
        }
    }

    #[test_case]
    fn test_pipeline_validation() {
        let mut pipeline = FlexiblePipeline::new();

        // Empty pipeline should validate
        assert!(pipeline.validate().is_ok());

        // Add stage with no processors - should fail validation
        let empty_stage = FlexibleStage::new("empty");
        pipeline.add_stage(empty_stage).unwrap();

        let result = pipeline.validate();
        assert!(result.is_err());
        match result {
            Err(NetworkError::InvalidStageConfig(msg)) => {
                assert!(msg.contains("empty"));
                assert!(msg.contains("no processors"));
            }
            _ => panic!("Expected InvalidStageConfig error"),
        }

        // Add processor to make it valid
        let stage = pipeline.get_stage_mut("empty").unwrap();
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockHandler::new("test")),
            NextAction::Complete,
        ));

        assert!(pipeline.validate().is_ok());

        // Set non-existent stage as default - should fail
        pipeline.set_default_entry_stage("empty").unwrap();
        pipeline.remove_stage("empty");

        let result = pipeline.validate();
        assert!(result.is_err());
        match result {
            Err(NetworkError::InvalidStageConfig(msg)) => {
                assert!(msg.contains("Default entry stage"));
                assert!(msg.contains("does not exist"));
            }
            _ => panic!("Expected InvalidStageConfig error"),
        }
    }

    #[test_case]
    fn test_stage_operations() {
        let mut stage = FlexibleStage::new("test");
        
        // Add some processors
        for i in 0..3 {
            stage.add_processor(StageProcessor::new(
                Box::new(MockCondition { should_match: false }),
                Box::new(MockHandler::new(&alloc::format!("handler{}", i))),
                NextAction::Complete,
            ));
        }

        assert_eq!(stage.processor_count(), 3);

        stage.clear_processors();
        assert_eq!(stage.processor_count(), 0);
    }

    #[test_case]
    fn test_pipeline_stage_removal() {
        let mut pipeline = FlexiblePipeline::new();
        
        let stage = FlexibleStage::new("test");
        pipeline.add_stage(stage).unwrap();
        assert!(pipeline.has_stage("test"));

        let removed = pipeline.remove_stage("test");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().stage_id, "test");
        assert!(!pipeline.has_stage("test"));

        // Removing non-existent stage should return None
        let removed = pipeline.remove_stage("nonexistent");
        assert!(removed.is_none());
    }
}