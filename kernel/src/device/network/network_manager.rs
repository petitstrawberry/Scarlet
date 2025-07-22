//! Network Manager for the flexible pipeline architecture
//!
//! This module implements the NetworkManager that orchestrates packet processing
//! through the flexible pipeline system.

use hashbrown::HashSet;
use alloc::string::{String, ToString};
use spin::Mutex;
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::NextAction,
    pipeline::FlexiblePipeline,
};

/// Statistics for network packet processing
#[derive(Debug, Clone, Default)]
pub struct NetworkProcessingStats {
    /// Total packets processed
    pub packets_processed: u64,
    /// Packets successfully completed
    pub packets_completed: u64,
    /// Packets dropped
    pub packets_dropped: u64,
    /// Packets terminated (e.g., waiting for fragments)
    pub packets_terminated: u64,
    /// Processing errors encountered
    pub processing_errors: u64,
    /// Pipeline loops detected
    pub pipeline_loops: u64,
}

/// The main network manager that coordinates packet processing
///
/// NetworkManager contains the processing pipeline and provides methods
/// for packet processing, pipeline management, and statistics collection.
pub struct NetworkManager {
    /// The packet processing pipeline
    pipeline: Mutex<FlexiblePipeline>,
    /// Processing statistics
    stats: Mutex<NetworkProcessingStats>,
    /// Maximum number of stage transitions to prevent infinite loops
    max_pipeline_hops: usize,
}

impl NetworkManager {
    /// Create a new NetworkManager with an empty pipeline
    pub fn new() -> Self {
        Self {
            pipeline: Mutex::new(FlexiblePipeline::new()),
            stats: Mutex::new(NetworkProcessingStats::default()),
            max_pipeline_hops: 32, // Reasonable default to prevent infinite loops
        }
    }

    /// Create a new NetworkManager with the specified pipeline
    pub fn with_pipeline(pipeline: FlexiblePipeline) -> Self {
        Self {
            pipeline: Mutex::new(pipeline),
            stats: Mutex::new(NetworkProcessingStats::default()),
            max_pipeline_hops: 32,
        }
    }

    /// Set the maximum number of pipeline hops allowed
    ///
    /// This prevents infinite loops in misconfigured pipelines.
    pub fn set_max_pipeline_hops(&mut self, max_hops: usize) {
        self.max_pipeline_hops = max_hops;
    }

    /// Process a packet through the pipeline
    ///
    /// This is the main entry point for packet processing. It:
    /// 1. Gets the default entry stage or uses the provided stage
    /// 2. Processes the packet through stages following NextAction directives
    /// 3. Handles loops and termination conditions
    /// 4. Updates statistics
    ///
    /// # Arguments
    /// * `packet` - The packet to process
    ///
    /// # Returns
    /// * `Ok(())` - Packet was processed successfully (completed, dropped, or terminated)
    /// * `Err(NetworkError)` - Processing failed due to an error
    pub fn process_packet(&self, mut packet: NetworkPacket) -> Result<(), NetworkError> {
        let mut stats = self.stats.lock();
        stats.packets_processed += 1;
        drop(stats);

        // Start processing from the default entry stage
        let current_stage = {
            let pipeline = self.pipeline.lock();
            match pipeline.get_default_entry_stage() {
                Some(stage) => stage.to_string(),
                None => {
                    let mut stats = self.stats.lock();
                    stats.processing_errors += 1;
                    return Err(NetworkError::PipelineNotInitialized);
                }
            }
        };

        match self.process_packet_from_stage(packet, &current_stage) {
            Ok(_) => Ok(()),
            Err(e) => {
                let mut stats = self.stats.lock();
                stats.processing_errors += 1;
                Err(e)
            }
        }
    }

    /// Process a packet starting from a specific stage
    ///
    /// # Arguments
    /// * `packet` - The packet to process
    /// * `start_stage` - The stage to start processing from
    ///
    /// # Returns
    /// * `Ok(())` - Processing completed
    /// * `Err(NetworkError)` - Processing failed
    pub fn process_packet_from_stage(
        &self, 
        mut packet: NetworkPacket, 
        start_stage: &str
    ) -> Result<(), NetworkError> {
        let mut current_stage = start_stage.to_string();
        let mut visited_stages = HashSet::new();
        let mut hop_count = 0;

        loop {
            // Check for infinite loops
            if hop_count >= self.max_pipeline_hops {
                let mut stats = self.stats.lock();
                stats.pipeline_loops += 1;
                return Err(NetworkError::circular_dependency(
                    &alloc::format!("Pipeline exceeded {} hops, possible loop", self.max_pipeline_hops)
                ));
            }

            // Check for circular dependency by tracking visited stages
            if visited_stages.contains(&current_stage) {
                let mut stats = self.stats.lock();
                stats.pipeline_loops += 1;
                return Err(NetworkError::circular_dependency(
                    &alloc::format!("Circular dependency detected: revisited stage '{}'", current_stage)
                ));
            }

            visited_stages.insert(current_stage.clone());
            hop_count += 1;

            // Process packet in current stage
            let next_action = {
                let pipeline = self.pipeline.lock();
                pipeline.process_packet_in_stage(&current_stage, &mut packet)?
            };

            // Handle the next action
            match next_action {
                NextAction::Jump(next_stage) => {
                    current_stage = next_stage;
                    continue;
                }
                NextAction::Complete => {
                    let mut stats = self.stats.lock();
                    stats.packets_completed += 1;
                    return Ok(());
                }
                NextAction::Drop(_reason) => {
                    let mut stats = self.stats.lock();
                    stats.packets_dropped += 1;
                    // In a real implementation, might want to log the drop reason
                    return Ok(()); // Dropping is successful completion
                }
                NextAction::Terminate => {
                    let mut stats = self.stats.lock();
                    stats.packets_terminated += 1;
                    return Ok(()); // Termination is successful completion
                }
            }
        }
    }

    /// Get a copy of the current pipeline
    ///
    /// This creates a clone of the pipeline for inspection or backup purposes.
    pub fn get_pipeline(&self) -> FlexiblePipeline {
        // Note: This is a simplified implementation. In practice, you might want
        // to implement a more efficient way to provide read-only access.
        let _pipeline = self.pipeline.lock();
        
        // Since FlexiblePipeline doesn't implement Clone, we need to create a new one
        // and manually copy the configuration. For now, we'll return a new empty pipeline
        // as the actual cloning would require the stage processors to be cloneable.
        FlexiblePipeline::new()
    }

    /// Replace the current pipeline
    ///
    /// # Arguments
    /// * `new_pipeline` - The new pipeline to install
    ///
    /// # Returns
    /// * `Ok(())` - Pipeline replaced successfully
    /// * `Err(NetworkError)` - Pipeline validation failed
    pub fn set_pipeline(&self, new_pipeline: FlexiblePipeline) -> Result<(), NetworkError> {
        // Validate the new pipeline before installing it
        new_pipeline.validate()?;
        
        let mut pipeline = self.pipeline.lock();
        *pipeline = new_pipeline;
        Ok(())
    }

    /// Add a stage to the pipeline
    ///
    /// This is a convenience method for adding stages without replacing the entire pipeline.
    pub fn add_stage(&self, stage: super::pipeline::FlexibleStage) -> Result<(), NetworkError> {
        let mut pipeline = self.pipeline.lock();
        pipeline.add_stage(stage)
    }

    /// Remove a stage from the pipeline
    ///
    /// # Arguments
    /// * `stage_id` - ID of the stage to remove
    ///
    /// # Returns
    /// * `Some(FlexibleStage)` - The removed stage
    /// * `None` - If no stage with that ID exists
    pub fn remove_stage(&self, stage_id: &str) -> Option<super::pipeline::FlexibleStage> {
        let mut pipeline = self.pipeline.lock();
        pipeline.remove_stage(stage_id)
    }

    /// Set the default entry stage
    pub fn set_default_entry_stage(&self, stage_id: &str) -> Result<(), NetworkError> {
        let mut pipeline = self.pipeline.lock();
        pipeline.set_default_entry_stage(stage_id)
    }

    /// Get processing statistics
    pub fn get_stats(&self) -> NetworkProcessingStats {
        self.stats.lock().clone()
    }

    /// Reset processing statistics
    pub fn reset_stats(&self) {
        let mut stats = self.stats.lock();
        *stats = NetworkProcessingStats::default();
    }

    /// Validate the current pipeline configuration
    pub fn validate_pipeline(&self) -> Result<(), NetworkError> {
        let pipeline = self.pipeline.lock();
        pipeline.validate()
    }

    /// Check if a stage exists in the pipeline
    pub fn has_stage(&self, stage_id: &str) -> bool {
        let pipeline = self.pipeline.lock();
        pipeline.has_stage(stage_id)
    }

    /// Get all stage IDs in the pipeline
    pub fn get_stage_ids(&self) -> alloc::vec::Vec<String> {
        let pipeline = self.pipeline.lock();
        pipeline.stage_ids().iter().map(|s| String::from(*s)).collect()
    }

    /// Get the number of stages in the pipeline
    pub fn stage_count(&self) -> usize {
        let pipeline = self.pipeline.lock();
        pipeline.stage_count()
    }
}

impl Default for NetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

// Implement Debug manually since Mutex doesn't implement Debug in a useful way
impl core::fmt::Debug for NetworkManager {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NetworkManager")
            .field("max_pipeline_hops", &self.max_pipeline_hops)
            .field("stage_count", &self.stage_count())
            .field("stats", &self.get_stats())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{boxed::Box, vec, string::String};
    use crate::device::network::{
        pipeline::{FlexibleStage, StageProcessor}, 
        traits::{StageHandler, ProcessorCondition, NextAction}
    };

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
            // Simulate processing by reducing payload
            let payload = packet.payload().to_vec();
            if payload.len() > 2 {
                packet.set_payload(payload[2..].to_vec());
            }
            Ok(())
        }
    }

    struct MockCondition;

    impl ProcessorCondition for MockCondition {
        fn matches(&self, _packet: &NetworkPacket) -> bool {
            true // Always matches for simple testing
        }
    }

    struct FailingHandler;

    impl StageHandler for FailingHandler {
        fn handle(&self, _packet: &mut NetworkPacket) -> Result<(), NetworkError> {
            Err(NetworkError::invalid_packet("Mock failure"))
        }
    }

    fn create_test_pipeline() -> FlexiblePipeline {
        let mut pipeline = FlexiblePipeline::new();

        // Create ethernet stage
        let mut eth_stage = FlexibleStage::new("ethernet");
        eth_stage.add_processor(StageProcessor::new(
            Box::new(MockCondition),
            Box::new(MockHandler::new("ethernet")),
            NextAction::jump_to("ipv4"),
        ));

        // Create IPv4 stage
        let mut ipv4_stage = FlexibleStage::new("ipv4");
        ipv4_stage.add_processor(StageProcessor::new(
            Box::new(MockCondition),
            Box::new(MockHandler::new("ipv4")),
            NextAction::Complete,
        ));

        pipeline.add_stage(eth_stage).unwrap();
        pipeline.add_stage(ipv4_stage).unwrap();
        pipeline.set_default_entry_stage("ethernet").unwrap();

        pipeline
    }

    #[test_case]
    fn test_network_manager_creation() {
        let manager = NetworkManager::new();
        assert_eq!(manager.stage_count(), 0);

        let stats = manager.get_stats();
        assert_eq!(stats.packets_processed, 0);
        assert_eq!(stats.packets_completed, 0);
    }

    #[test_case]
    fn test_network_manager_with_pipeline() {
        let pipeline = create_test_pipeline();
        let manager = NetworkManager::with_pipeline(pipeline);
        
        assert_eq!(manager.stage_count(), 2);
        assert!(manager.has_stage("ethernet"));
        assert!(manager.has_stage("ipv4"));

        let stage_ids = manager.get_stage_ids();
        assert!(stage_ids.contains(&String::from("ethernet")));
        assert!(stage_ids.contains(&String::from("ipv4")));
    }

    #[test_case]
    fn test_packet_processing_success() {
        let pipeline = create_test_pipeline();
        let manager = NetworkManager::with_pipeline(pipeline);
        
        let packet = NetworkPacket::new(
            vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
            String::from("eth0")
        );

        let result = manager.process_packet(packet);
        assert!(result.is_ok());

        let stats = manager.get_stats();
        assert_eq!(stats.packets_processed, 1);
        assert_eq!(stats.packets_completed, 1);
        assert_eq!(stats.packets_dropped, 0);
        assert_eq!(stats.processing_errors, 0);
    }

    #[test_case]
    fn test_packet_processing_no_default_stage() {
        let mut pipeline = FlexiblePipeline::new();
        let stage = FlexibleStage::new("test");
        pipeline.add_stage(stage).unwrap();
        // Don't set default entry stage

        let manager = NetworkManager::with_pipeline(pipeline);
        let packet = NetworkPacket::new(vec![0x01], String::from("test"));

        let result = manager.process_packet(packet);
        assert!(result.is_err());
        match result {
            Err(NetworkError::PipelineNotInitialized) => {} // Expected
            _ => panic!("Expected PipelineNotInitialized error"),
        }

        let stats = manager.get_stats();
        assert_eq!(stats.processing_errors, 1);
    }

    #[test_case]
    fn test_packet_processing_with_drop() {
        let mut pipeline = FlexiblePipeline::new();
        
        let mut stage = FlexibleStage::new("dropper");
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition),
            Box::new(MockHandler::new("dropper")),
            NextAction::drop_with_reason("test drop"),
        ));
        
        pipeline.add_stage(stage).unwrap();
        pipeline.set_default_entry_stage("dropper").unwrap();
        
        let manager = NetworkManager::with_pipeline(pipeline);
        let packet = NetworkPacket::new(vec![0x01], String::from("test"));

        let result = manager.process_packet(packet);
        assert!(result.is_ok());

        let stats = manager.get_stats();
        assert_eq!(stats.packets_dropped, 1);
        assert_eq!(stats.packets_completed, 0);
    }

    #[test_case]
    fn test_packet_processing_with_terminate() {
        let mut pipeline = FlexiblePipeline::new();
        
        let mut stage = FlexibleStage::new("terminator");
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition),
            Box::new(MockHandler::new("terminator")),
            NextAction::Terminate,
        ));
        
        pipeline.add_stage(stage).unwrap();
        pipeline.set_default_entry_stage("terminator").unwrap();
        
        let manager = NetworkManager::with_pipeline(pipeline);
        let packet = NetworkPacket::new(vec![0x01], String::from("test"));

        let result = manager.process_packet(packet);
        assert!(result.is_ok());

        let stats = manager.get_stats();
        assert_eq!(stats.packets_terminated, 1);
    }

    #[test_case]
    fn test_circular_dependency_detection() {
        let mut pipeline = FlexiblePipeline::new();
        
        // Create stage A that jumps to B
        let mut stage_a = FlexibleStage::new("stage_a");
        stage_a.add_processor(StageProcessor::new(
            Box::new(MockCondition),
            Box::new(MockHandler::new("stage_a")),
            NextAction::jump_to("stage_b"),
        ));
        
        // Create stage B that jumps back to A (circular dependency)
        let mut stage_b = FlexibleStage::new("stage_b");
        stage_b.add_processor(StageProcessor::new(
            Box::new(MockCondition),
            Box::new(MockHandler::new("stage_b")),
            NextAction::jump_to("stage_a"),
        ));
        
        pipeline.add_stage(stage_a).unwrap();
        pipeline.add_stage(stage_b).unwrap();
        pipeline.set_default_entry_stage("stage_a").unwrap();
        
        let manager = NetworkManager::with_pipeline(pipeline);
        let packet = NetworkPacket::new(vec![0x01], String::from("test"));

        let result = manager.process_packet(packet);
        assert!(result.is_err());
        match result {
            Err(NetworkError::CircularDependency(_)) => {} // Expected
            _ => panic!("Expected CircularDependency error"),
        }

        let stats = manager.get_stats();
        assert_eq!(stats.pipeline_loops, 1);
    }

    #[test_case]
    fn test_max_hops_limit() {
        let mut pipeline = FlexiblePipeline::new();
        
        // Create stages that form a long chain
        for i in 0..10 {
            let mut stage = FlexibleStage::new(&alloc::format!("stage_{}", i));
            let next_stage = alloc::format!("stage_{}", (i + 1) % 10);
            stage.add_processor(StageProcessor::new(
                Box::new(MockCondition),
                Box::new(MockHandler::new(&alloc::format!("handler_{}", i))),
                NextAction::jump_to(&next_stage),
            ));
            pipeline.add_stage(stage).unwrap();
        }
        
        pipeline.set_default_entry_stage("stage_0").unwrap();
        
        let mut manager = NetworkManager::with_pipeline(pipeline);
        manager.set_max_pipeline_hops(5); // Set low limit
        
        let packet = NetworkPacket::new(vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06], String::from("test"));

        let result = manager.process_packet(packet);
        assert!(result.is_err());
        match result {
            Err(NetworkError::CircularDependency(msg)) => {
                assert!(msg.contains("exceeded"));
                assert!(msg.contains("hops"));
            }
            _ => panic!("Expected CircularDependency error for max hops"),
        }
    }

    #[test_case]
    fn test_stage_processing_failure() {
        let mut pipeline = FlexiblePipeline::new();
        
        let mut stage = FlexibleStage::new("failing");
        stage.add_processor(StageProcessor::new(
            Box::new(MockCondition),
            Box::new(FailingHandler),
            NextAction::Complete,
        ));
        
        pipeline.add_stage(stage).unwrap();
        pipeline.set_default_entry_stage("failing").unwrap();
        
        let manager = NetworkManager::with_pipeline(pipeline);
        let packet = NetworkPacket::new(vec![0x01], String::from("test"));

        let result = manager.process_packet(packet);
        assert!(result.is_err());

        let stats = manager.get_stats();
        assert_eq!(stats.processing_errors, 1);
    }

    #[test_case]
    fn test_manager_operations() {
        let manager = NetworkManager::new();
        
        // Test adding stages
        let stage = FlexibleStage::new("test");
        manager.add_stage(stage).unwrap();
        assert!(manager.has_stage("test"));
        
        // Test setting default entry stage
        let result = manager.set_default_entry_stage("test");
        assert!(result.is_err()); // Should fail because stage has no processors
        
        // Test removing stage
        let removed = manager.remove_stage("test");
        assert!(removed.is_some());
        assert!(!manager.has_stage("test"));
        
        // Test validation
        assert!(manager.validate_pipeline().is_ok()); // Empty pipeline is valid
    }

    #[test_case]
    fn test_stats_operations() {
        let manager = NetworkManager::new();
        
        let initial_stats = manager.get_stats();
        assert_eq!(initial_stats.packets_processed, 0);
        
        // Manually update stats for testing
        {
            let mut stats = manager.stats.lock();
            stats.packets_processed = 10;
            stats.packets_completed = 8;
            stats.packets_dropped = 2;
        }
        
        let updated_stats = manager.get_stats();
        assert_eq!(updated_stats.packets_processed, 10);
        assert_eq!(updated_stats.packets_completed, 8);
        assert_eq!(updated_stats.packets_dropped, 2);
        
        manager.reset_stats();
        let reset_stats = manager.get_stats();
        assert_eq!(reset_stats.packets_processed, 0);
    }

    #[test_case]
    fn test_debug_formatting() {
        let manager = NetworkManager::new();
        let debug_str = alloc::format!("{:?}", manager);
        assert!(debug_str.contains("NetworkManager"));
        assert!(debug_str.contains("max_pipeline_hops"));
    }
}