use crate::network::*;
use crate::network::test_helpers::*;
use alloc::string::ToString;
use alloc::vec;

#[test_case]
fn test_network_packet_creation() {
    let payload = vec![1, 2, 3, 4, 5];
    let packet = NetworkPacket::new(payload.clone());
    
    assert_eq!(packet.payload(), &payload);
    assert_eq!(packet.total_size(), 5);
}

#[test_case]
fn test_network_packet_headers() {
    let mut packet = NetworkPacket::new(vec![1, 2, 3]);
    
    // Add headers
    packet.add_header("ethernet", vec![0x00, 0x11, 0x22]);
    packet.add_header("ipv4", vec![0x45, 0x00]);
    
    assert_eq!(packet.get_header("ethernet"), Some(&vec![0x00, 0x11, 0x22]));
    assert_eq!(packet.get_header("ipv4"), Some(&vec![0x45, 0x00]));
    assert_eq!(packet.get_header("tcp"), None);
    
    // Check total size
    assert_eq!(packet.total_size(), 3 + 3 + 2); // payload + ethernet + ipv4
}

#[test_case]
fn test_network_packet_hints() {
    let mut packet = NetworkPacket::new(vec![1, 2, 3]);
    
    // Set hints
    packet.set_hint("ethertype", "0x0800");
    packet.set_hint("dest_ip", "192.168.1.100");
    
    assert_eq!(packet.get_hint("ethertype"), Some("0x0800"));
    assert_eq!(packet.get_hint("dest_ip"), Some("192.168.1.100"));
    assert_eq!(packet.get_hint("nonexistent"), None);
}

#[test_case]
fn test_flexible_pipeline_builder() {
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("test1")
                .enable_rx()
                .enable_tx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("test2")
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("test1")
        .set_default_tx_entry("test1")
        .build()
        .expect("Pipeline build should succeed");

    assert!(pipeline.has_stage("test1"));
    assert!(pipeline.has_stage("test2"));
    assert!(!pipeline.has_stage("nonexistent"));
    
    let stage_ids = pipeline.stage_ids();
    assert_eq!(stage_ids.len(), 2);
    assert!(stage_ids.contains(&"test1".to_string()));
    assert!(stage_ids.contains(&"test2".to_string()));
}

#[test_case]
fn test_flexible_pipeline_receive() {
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("echo")
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("echo")
        .build()
        .expect("Pipeline build should succeed");

    let packet = NetworkPacket::new(vec![1, 2, 3, 4]);
    let result = pipeline.process_receive(packet, None);
    
    assert!(result.is_ok());
    let processed_packet = result.unwrap();
    assert_eq!(processed_packet.payload(), &vec![1, 2, 3, 4]);
}

#[test_case]
fn test_flexible_pipeline_transmit() {
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("echo")
                .enable_tx()
                .build()
        )
        .set_default_tx_entry("echo")
        .build()
        .expect("Pipeline build should succeed");

    let packet = NetworkPacket::new(vec![5, 6, 7, 8]);
    let result = pipeline.process_transmit(packet, None);
    
    assert!(result.is_ok());
    let processed_packet = result.unwrap();
    assert_eq!(processed_packet.payload(), &vec![5, 6, 7, 8]);
}

#[test_case]
fn test_pipeline_errors() {
    let pipeline = FlexiblePipeline::builder()
        .build()
        .expect("Pipeline build should succeed");

    let packet = NetworkPacket::new(vec![1, 2, 3]);
    
    // No stages defined, should return error
    let result = pipeline.process_receive(packet.clone(), None);
    assert!(result.is_err());

    // Nonexistent stage specified
    let result = pipeline.process_receive(packet, Some("nonexistent"));
    assert!(result.is_err());
}
