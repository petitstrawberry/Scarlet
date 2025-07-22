//! Pipeline infrastructure for flexible network packet processing
//!
//! This module implements the core pipeline components that enable flexible,
//! protocol-independent packet processing through Tx/Rx separated staged processors.

use hashbrown::HashMap;
use alloc::{
    boxed::Box, format, string::String, vec::Vec
};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::{RxStageHandler, TxStageBuilder, ProcessorCondition, NextAction},
};

/// A single processor within a pipeline stage for receive path
///
/// Each RxStageProcessor combines:
/// - A condition that determines if this processor should handle a packet
/// - A handler that performs the actual packet processing for receive path
/// - An action that specifies what to do after processing
pub struct RxStageProcessor {
    /// Condition to check if this processor should handle the packet
    pub condition: Box<dyn ProcessorCondition>,
    /// Handler to process the packet if condition matches (receive path)
    pub handler: Box<dyn RxStageHandler>,
    /// Action to take after successful processing
    pub next_action: NextAction,
}

impl RxStageProcessor {
    /// Create a new receive stage processor
    ///
    /// # Arguments
    /// * `condition` - Condition checker for this processor
    /// * `handler` - Packet handler for this processor  
    /// * `next_action` - Action to take after processing
    pub fn new(
        condition: Box<dyn ProcessorCondition>,
        handler: Box<dyn RxStageHandler>,
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

/// A single processor within a pipeline stage for transmit path
///
/// Each TxStageProcessor combines:
/// - A condition that determines if this processor should handle a packet
/// - A builder that performs the actual packet building for transmit path
/// - An action that specifies what to do after processing
pub struct TxStageProcessor {
    /// Condition to check if this processor should handle the packet
    pub condition: Box<dyn ProcessorCondition>,
    /// Builder to build the packet if condition matches (transmit path)
    pub builder: Box<dyn TxStageBuilder>,
    /// Action to take after successful processing
    pub next_action: NextAction,
}

impl TxStageProcessor {
    /// Create a new transmit stage processor
    ///
    /// # Arguments
    /// * `condition` - Condition checker for this processor
    /// * `builder` - Packet builder for this processor  
    /// * `next_action` - Action to take after processing
    pub fn new(
        condition: Box<dyn ProcessorCondition>,
        builder: Box<dyn TxStageBuilder>,
        next_action: NextAction,
    ) -> Self {
        Self {
            condition,
            builder,
            next_action,
        }
    }

    /// Check if this processor should handle the given packet
    pub fn matches(&self, packet: &NetworkPacket) -> bool {
        self.condition.matches(packet)
    }

    /// Process the packet with this processor's builder
    pub fn process(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        self.builder.build(packet)?;
        Ok(self.next_action.clone())
    }
}

/// A stage in the processing pipeline with Tx/Rx separation
///
/// Each FlexibleStage represents a protocol layer (e.g., ethernet, ipv4, tcp)
/// and contains separate processors for receive and transmit paths that can
/// handle different packet variants within that layer.
pub struct FlexibleStage {
    /// Unique identifier for this stage
    pub stage_id: String,
    /// List of receive path processors in this stage (tried in order)
    pub rx_processors: Vec<RxStageProcessor>,
    /// List of transmit path processors in this stage (tried in order)
    pub tx_processors: Vec<TxStageProcessor>,
}

impl FlexibleStage {
    /// Create a new flexible stage
    ///
    /// # Arguments
    /// * `stage_id` - Unique identifier for this stage
    pub fn new(stage_id: &str) -> Self {
        Self {
            stage_id: String::from(stage_id),
            rx_processors: Vec::new(),
            tx_processors: Vec::new(),
        }
    }

    /// Add a receive processor to this stage
    ///
    /// Processors are tried in the order they are added.
    /// The first processor whose condition matches will handle the packet.
    pub fn add_rx_processor(&mut self, processor: RxStageProcessor) {
        self.rx_processors.push(processor);
    }

    /// Add a transmit processor to this stage
    ///
    /// Processors are tried in the order they are added.
    /// The first processor whose condition matches will handle the packet.
    pub fn add_tx_processor(&mut self, processor: TxStageProcessor) {
        self.tx_processors.push(processor);
    }

    /// Remove all receive processors from this stage
    pub fn clear_rx_processors(&mut self) {
        self.rx_processors.clear();
    }

    /// Remove all transmit processors from this stage
    pub fn clear_tx_processors(&mut self) {
        self.tx_processors.clear();
    }

    /// Get the number of receive processors in this stage
    pub fn rx_processor_count(&self) -> usize {
        self.rx_processors.len()
    }

    /// Get the number of transmit processors in this stage
    pub fn tx_processor_count(&self) -> usize {
        self.tx_processors.len()
    }

    /// Get the total number of processors (rx + tx) in this stage
    pub fn total_processor_count(&self) -> usize {
        self.rx_processors.len() + self.tx_processors.len()
    }

    /// Process a received packet through this stage
    ///
    /// Tries receive processors in order until one matches and processes the packet.
    /// Returns the NextAction from the matching processor.
    ///
    /// # Arguments
    /// * `packet` - The packet to process
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take after processing
    /// * `Err(NetworkError)` - If no processor matches or processing fails
    pub fn process_rx_packet(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        for processor in &self.rx_processors {
            if processor.matches(packet) {
                return processor.process(packet);
            }
        }
        
        Err(NetworkError::no_matching_processor(&format!("Stage {} (rx): No matching processor", self.stage_id)))
    }

    /// Process a transmit packet through this stage
    ///
    /// Tries transmit processors in order until one matches and processes the packet.
    /// Returns the NextAction from the matching processor.
    ///
    /// # Arguments
    /// * `packet` - The packet to process
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take after processing
    /// * `Err(NetworkError)` - If no processor matches or processing fails
    pub fn process_tx_packet(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        for processor in &self.tx_processors {
            if processor.matches(packet) {
                return processor.process(packet);
            }
        }
        
        Err(NetworkError::no_matching_processor(&alloc::format!("{} (tx)", self.stage_id)))
    }
}

/// The main pipeline that orchestrates packet processing through stages
///
/// FlexiblePipeline manages a collection of stages and provides methods
/// for adding, removing, and organizing stages for Tx/Rx separated packet processing.
pub struct FlexiblePipeline {
    /// Map of stage ID to stage implementation
    stages: HashMap<String, FlexibleStage>,
    /// Default starting stage for receive packet processing
    default_rx_entry_stage: Option<String>,
    /// Default starting stage for transmit packet processing  
    default_tx_entry_stage: Option<String>,
}

impl FlexiblePipeline {
    /// Create a new empty pipeline
    pub fn new() -> Self {
        Self {
            stages: HashMap::new(),
            default_rx_entry_stage: None,
            default_tx_entry_stage: None,
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

    /// Set the default receive entry stage for packet processing
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to use as default receive entry point
    ///
    /// # Returns
    /// * `Ok(())` if successful
    /// * `Err(NetworkError)` if the stage doesn't exist or has no receive processors
    pub fn set_default_rx_entry_stage(&mut self, stage_id: &str) -> Result<(), NetworkError> {
        if !self.stages.contains_key(stage_id) {
            return Err(NetworkError::stage_not_found(stage_id));
        }
        
        // Check if the stage has at least one receive processor
        let stage = &self.stages[stage_id];
        if stage.rx_processor_count() == 0 {
            return Err(NetworkError::invalid_stage_config(
                &alloc::format!("Stage '{}' has no receive processors and cannot be used as default rx entry", stage_id)
            ));
        }
        
        self.default_rx_entry_stage = Some(String::from(stage_id));
        Ok(())
    }

    /// Set the default transmit entry stage for packet processing
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to use as default transmit entry point
    ///
    /// # Returns
    /// * `Ok(())` if successful
    /// * `Err(NetworkError)` if the stage doesn't exist or has no transmit processors
    pub fn set_default_tx_entry_stage(&mut self, stage_id: &str) -> Result<(), NetworkError> {
        if !self.stages.contains_key(stage_id) {
            return Err(NetworkError::stage_not_found(stage_id));
        }
        
        // Check if the stage has at least one transmit processor
        let stage = &self.stages[stage_id];
        if stage.tx_processor_count() == 0 {
            return Err(NetworkError::invalid_stage_config(
                &alloc::format!("Stage '{}' has no transmit processors and cannot be used as default tx entry", stage_id)
            ));
        }
        
        self.default_tx_entry_stage = Some(String::from(stage_id));
        Ok(())
    }

    /// Get the default receive entry stage ID
    pub fn get_default_rx_entry_stage(&self) -> Option<&str> {
        self.default_rx_entry_stage.as_deref()
    }

    /// Get the default transmit entry stage ID
    pub fn get_default_tx_entry_stage(&self) -> Option<&str> {
        self.default_tx_entry_stage.as_deref()
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

    /// Process a received packet through a specific stage
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to process with
    /// * `packet` - The packet to process
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take after processing
    /// * `Err(NetworkError)` - If stage not found or processing fails
    pub fn process_rx_packet_in_stage(
        &self, 
        stage_id: &str, 
        packet: &mut NetworkPacket
    ) -> Result<NextAction, NetworkError> {
        let stage = self.stages.get(stage_id)
            .ok_or_else(|| NetworkError::stage_not_found(stage_id))?;
        
        stage.process_rx_packet(packet)
    }

    /// Process a transmit packet through a specific stage
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to process with
    /// * `packet` - The packet to process
    ///
    /// # Returns
    /// * `Ok(NextAction)` - Action to take after processing
    /// * `Err(NetworkError)` - If stage not found or processing fails
    pub fn process_tx_packet_in_stage(
        &self, 
        stage_id: &str, 
        packet: &mut NetworkPacket
    ) -> Result<NextAction, NetworkError> {
        let stage = self.stages.get(stage_id)
            .ok_or_else(|| NetworkError::stage_not_found(stage_id))?;
        
        stage.process_tx_packet(packet)
    }

    /// Validate pipeline configuration
    ///
    /// Checks for common configuration errors like circular dependencies.
    /// This is a basic implementation - a full version might do more sophisticated
    /// dependency analysis.
    pub fn validate(&self) -> Result<(), NetworkError> {
        // Basic validation: check that default entry stages exist
        if let Some(rx_entry_stage) = &self.default_rx_entry_stage {
            if !self.stages.contains_key(rx_entry_stage) {
                return Err(NetworkError::invalid_stage_config(
                    &alloc::format!("Default rx entry stage '{}' does not exist", rx_entry_stage)
                ));
            }
        }
        
        if let Some(tx_entry_stage) = &self.default_tx_entry_stage {
            if !self.stages.contains_key(tx_entry_stage) {
                return Err(NetworkError::invalid_stage_config(
                    &alloc::format!("Default tx entry stage '{}' does not exist", tx_entry_stage)
                ));
            }
        }

        // Check that all stages have at least one processor (rx or tx)
        for (stage_id, stage) in &self.stages {
            if stage.total_processor_count() == 0 {
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
impl core::fmt::Debug for RxStageProcessor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RxStageProcessor")
            .field("next_action", &self.next_action)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for TxStageProcessor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TxStageProcessor")
            .field("next_action", &self.next_action)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for FlexibleStage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FlexibleStage")
            .field("stage_id", &self.stage_id)
            .field("rx_processor_count", &self.rx_processors.len())
            .field("tx_processor_count", &self.tx_processors.len())
            .finish()
    }
}

impl core::fmt::Debug for FlexiblePipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FlexiblePipeline")
            .field("stage_count", &self.stages.len())
            .field("default_rx_entry_stage", &self.default_rx_entry_stage)
            .field("default_tx_entry_stage", &self.default_tx_entry_stage)
            .field("stages", &self.stages.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // Mock implementations for testing

    struct MockRxHandler {
        header_name: String,
    }

    impl MockRxHandler {
        fn new(header_name: &str) -> Self {
            Self { header_name: String::from(header_name) }
        }
    }

    impl RxStageHandler for MockRxHandler {
        fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            packet.add_header(&self.header_name, vec![0x01, 0x02]);
            Ok(())
        }
    }

    struct MockTxBuilder {
        header_bytes: Vec<u8>,
    }

    impl MockTxBuilder {
        fn new(header_bytes: Vec<u8>) -> Self {
            Self { header_bytes }
        }
    }

    impl TxStageBuilder for MockTxBuilder {
        fn build(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            // Prepend header to payload
            let mut new_payload = self.header_bytes.clone();
            new_payload.extend_from_slice(packet.payload());
            packet.set_payload(new_payload);
            packet.set_hint("processed", "true");
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
    fn test_rx_stage_processor_creation() {
        let processor = RxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockRxHandler::new("test")),
            NextAction::Complete,
        );

        let packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        assert!(processor.matches(&packet));
    }

    #[test_case]
    fn test_tx_stage_processor_creation() {
        let processor = TxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockTxBuilder::new(vec![0xBB, 0xCC])),
            NextAction::Complete,
        );

        let packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        assert!(processor.matches(&packet));
    }

    #[test_case]
    fn test_rx_stage_processor_processing() {
        let processor = RxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockRxHandler::new("test")),
            NextAction::jump_to("next"),
        );

        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = processor.process(&mut packet);
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::jump_to("next"));
        assert_eq!(packet.get_header("test"), Some(&[0x01, 0x02][..]));
    }

    #[test_case]
    fn test_tx_stage_processor_processing() {
        let processor = TxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockTxBuilder::new(vec![0xBB, 0xCC])),
            NextAction::jump_to("next"),
        );

        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = processor.process(&mut packet);
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::jump_to("next"));
        assert_eq!(packet.payload(), &[0xBB, 0xCC, 0xAA]); // Header prepended
        assert_eq!(packet.get_hint("processed"), Some("true"));
    }

    #[test_case]
    fn test_flexible_stage() {
        let mut stage = FlexibleStage::new("test_stage");
        assert_eq!(stage.stage_id, "test_stage");
        assert_eq!(stage.rx_processor_count(), 0);
        assert_eq!(stage.tx_processor_count(), 0);
        assert_eq!(stage.total_processor_count(), 0);

        // Add rx processors
        stage.add_rx_processor(RxStageProcessor::new(
            Box::new(MockCondition { should_match: false }),
            Box::new(MockRxHandler::new("handler1")),
            NextAction::Complete,
        ));

        stage.add_rx_processor(RxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockRxHandler::new("handler2")),
            NextAction::jump_to("next"),
        ));

        // Add tx processor
        stage.add_tx_processor(TxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockTxBuilder::new(vec![0xDD, 0xEE])),
            NextAction::Complete,
        ));

        assert_eq!(stage.rx_processor_count(), 2);
        assert_eq!(stage.tx_processor_count(), 1);
        assert_eq!(stage.total_processor_count(), 3);

        // Test rx packet processing - should match second processor
        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = stage.process_rx_packet(&mut packet);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::jump_to("next"));
        assert_eq!(packet.get_header("handler2"), Some(&[0x01, 0x02][..]));
        assert!(packet.get_header("handler1").is_none()); // First processor didn't run

        // Test tx packet processing
        let mut packet = NetworkPacket::new(vec![0xBB], String::from("test"));
        let result = stage.process_tx_packet(&mut packet);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::Complete);
        assert_eq!(packet.payload(), &[0xDD, 0xEE, 0xBB]); // Header prepended
        assert_eq!(packet.get_hint("processed"), Some("true"));
    }

    #[test_case]
    fn test_flexible_stage_no_rx_match() {
        let mut stage = FlexibleStage::new("test_stage");
        stage.add_rx_processor(RxStageProcessor::new(
            Box::new(MockCondition { should_match: false }),
            Box::new(MockRxHandler::new("handler")),
            NextAction::Complete,
        ));

        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = stage.process_rx_packet(&mut packet);

        assert!(result.is_err());
        match result {
            Err(NetworkError::NoMatchingProcessor(stage_id)) => {
                assert_eq!(stage_id, "test_stage (rx)");
            }
            _ => panic!("Expected NoMatchingProcessor error"),
        }
    }

    #[test_case]
    fn test_flexible_stage_no_tx_match() {
        let mut stage = FlexibleStage::new("test_stage");
        stage.add_tx_processor(TxStageProcessor::new(
            Box::new(MockCondition { should_match: false }),
            Box::new(MockTxBuilder::new(vec![0xDD])),
            NextAction::Complete,
        ));

        let mut packet = NetworkPacket::new(vec![0xAA], String::from("test"));
        let result = stage.process_tx_packet(&mut packet);

        assert!(result.is_err());
        match result {
            Err(NetworkError::NoMatchingProcessor(stage_id)) => {
                assert_eq!(stage_id, "test_stage (tx)");
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
    fn test_pipeline_default_entry_stages() {
        let mut pipeline = FlexiblePipeline::new();
        assert!(pipeline.get_default_rx_entry_stage().is_none());
        assert!(pipeline.get_default_tx_entry_stage().is_none());

        // Try to set non-existent stage as default
        let result = pipeline.set_default_rx_entry_stage("ethernet");
        assert!(result.is_err());
        let result = pipeline.set_default_tx_entry_stage("ethernet");
        assert!(result.is_err());

        // Add empty stage and try to set as default - should fail
        let empty_stage = FlexibleStage::new("ethernet");
        pipeline.add_stage(empty_stage).unwrap();
        let result = pipeline.set_default_rx_entry_stage("ethernet");
        assert!(result.is_err()); // Should fail because stage has no rx processors
        let result = pipeline.set_default_tx_entry_stage("ethernet");
        assert!(result.is_err()); // Should fail because stage has no tx processors

        // Remove empty stage
        pipeline.remove_stage("ethernet");

        // Add stage with rx processor and set as default rx entry
        let mut stage = FlexibleStage::new("ethernet");
        stage.add_rx_processor(RxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockRxHandler::new("test")),
            NextAction::Complete,
        ));
        stage.add_tx_processor(TxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockTxBuilder::new(vec![0xEE, 0xFF])),
            NextAction::Complete,
        ));
        pipeline.add_stage(stage).unwrap();
        
        pipeline.set_default_rx_entry_stage("ethernet").unwrap();
        pipeline.set_default_tx_entry_stage("ethernet").unwrap();

        assert_eq!(pipeline.get_default_rx_entry_stage(), Some("ethernet"));
        assert_eq!(pipeline.get_default_tx_entry_stage(), Some("ethernet"));
    }

    #[test_case]
    fn test_pipeline_process_packets_in_stage() {
        let mut pipeline = FlexiblePipeline::new();
        
        // Create and add a stage with both rx and tx processors
        let mut stage = FlexibleStage::new("test");
        stage.add_rx_processor(RxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockRxHandler::new("test_header")),
            NextAction::Complete,
        ));
        stage.add_tx_processor(TxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockTxBuilder::new(vec![0xAA, 0xBB])),
            NextAction::Complete,
        ));
        pipeline.add_stage(stage).unwrap();

        // Test rx processing
        let mut packet = NetworkPacket::new(vec![0xCC], String::from("test"));
        let result = pipeline.process_rx_packet_in_stage("test", &mut packet);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::Complete);
        assert_eq!(packet.get_header("test_header"), Some(&[0x01, 0x02][..]));

        // Test tx processing  
        let mut packet = NetworkPacket::new(vec![0xDD], String::from("test"));
        let result = pipeline.process_tx_packet_in_stage("test", &mut packet);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NextAction::Complete);
        assert_eq!(packet.payload(), &[0xAA, 0xBB, 0xDD]); // Header prepended
        assert_eq!(packet.get_hint("processed"), Some("true"));

        // Test with non-existent stage
        let mut packet = NetworkPacket::new(vec![0xEE], String::from("test"));
        let result = pipeline.process_rx_packet_in_stage("nonexistent", &mut packet);
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
        stage.add_rx_processor(RxStageProcessor::new(
            Box::new(MockCondition { should_match: true }),
            Box::new(MockRxHandler::new("test")),
            NextAction::Complete,
        ));

        assert!(pipeline.validate().is_ok());

        // Set non-existent stage as default - should fail
        pipeline.set_default_rx_entry_stage("empty").unwrap();
        pipeline.remove_stage("empty");

        let result = pipeline.validate();
        assert!(result.is_err());
        match result {
            Err(NetworkError::InvalidStageConfig(msg)) => {
                assert!(msg.contains("Default rx entry stage"));
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
            stage.add_rx_processor(RxStageProcessor::new(
                Box::new(MockCondition { should_match: false }),
                Box::new(MockRxHandler::new(&alloc::format!("rx_handler{}", i))),
                NextAction::Complete,
            ));
            stage.add_tx_processor(TxStageProcessor::new(
                Box::new(MockCondition { should_match: false }),
                Box::new(MockTxBuilder::new(vec![i as u8])),
                NextAction::Complete,
            ));
        }

        assert_eq!(stage.rx_processor_count(), 3);
        assert_eq!(stage.tx_processor_count(), 3);
        assert_eq!(stage.total_processor_count(), 6);

        stage.clear_rx_processors();
        assert_eq!(stage.rx_processor_count(), 0);
        assert_eq!(stage.tx_processor_count(), 3);
        assert_eq!(stage.total_processor_count(), 3);

        stage.clear_tx_processors();
        assert_eq!(stage.rx_processor_count(), 0);
        assert_eq!(stage.tx_processor_count(), 0);
        assert_eq!(stage.total_processor_count(), 0);
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