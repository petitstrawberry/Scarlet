use crate::network::*;
use crate::network::test_helpers::*;
use alloc::string::ToString;
use alloc::vec;
use alloc::boxed::Box;

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

#[test_case]
fn test_payload_based_routing() {
    // Create matcher that routes based on first byte
    let matcher = PayloadByteMatcher::new("default")
        .add_route(0x08, "ipv4")
        .add_route(0x86, "ipv6");

    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            MatcherStageBuilder::new("ethernet")
                .with_payload_matcher(Box::new(matcher))
                .build()
        )
        .add_stage(
            TestStageBuilder::new("ipv4")
                .enable_rx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("ipv6")
                .enable_rx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("default")
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("ethernet")
        .build()
        .expect("Pipeline build should succeed");

    // Test IPv4 routing (first byte = 0x08)
    let packet_ipv4 = NetworkPacket::new(vec![0x08, 0x00, 0x01, 0x02]);
    let result = pipeline.process_receive(packet_ipv4, None);
    assert!(result.is_ok());

    // Test IPv6 routing (first byte = 0x86)
    let packet_ipv6 = NetworkPacket::new(vec![0x86, 0xDD, 0x03, 0x04]);
    let result = pipeline.process_receive(packet_ipv6, None);
    assert!(result.is_ok());

    // Test default routing (first byte = 0xFF)
    let packet_unknown = NetworkPacket::new(vec![0xFF, 0x00, 0x05, 0x06]);
    let result = pipeline.process_receive(packet_unknown, None);
    assert!(result.is_ok());
}

#[test_case]
fn test_hint_based_routing() {
    // Create matcher that routes based on "protocol" hint
    let matcher = HintMatcher::new("unknown")
        .add_route("tcp", "tcp_handler")
        .add_route("udp", "udp_handler");

    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            MatcherStageBuilder::new("transport")
                .with_hint_matcher("protocol", Box::new(matcher))
                .build()
        )
        .add_stage(
            TestStageBuilder::new("tcp_handler")
                .enable_rx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("udp_handler")
                .enable_rx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("unknown")
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("transport")
        .build()
        .expect("Pipeline build should succeed");

    // Test TCP routing
    let mut packet_tcp = NetworkPacket::new(vec![1, 2, 3, 4]);
    packet_tcp.set_hint("protocol", "tcp");
    let result = pipeline.process_receive(packet_tcp, None);
    assert!(result.is_ok());

    // Test UDP routing
    let mut packet_udp = NetworkPacket::new(vec![5, 6, 7, 8]);
    packet_udp.set_hint("protocol", "udp");
    let result = pipeline.process_receive(packet_udp, None);
    assert!(result.is_ok());

    // Test unknown protocol routing
    let mut packet_unknown = NetworkPacket::new(vec![9, 10, 11, 12]);
    packet_unknown.set_hint("protocol", "icmp");
    let result = pipeline.process_receive(packet_unknown, None);
    assert!(result.is_ok());
}

#[test_case]
fn test_multi_stage_pipeline() {
    // Create a 3-stage pipeline: ethernet -> ip -> transport
    let ethernet_matcher = PayloadByteMatcher::new("drop")
        .add_route(0x08, "ip_stage");

    let ip_matcher = PayloadByteMatcher::new("drop")
        .add_route(0x45, "transport_stage");

    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            MatcherStageBuilder::new("ethernet_stage")
                .with_payload_matcher(Box::new(ethernet_matcher))
                .build()
        )
        .add_stage(
            MatcherStageBuilder::new("ip_stage")
                .with_payload_matcher(Box::new(ip_matcher))
                .build()
        )
        .add_stage(
            TestStageBuilder::new("transport_stage")
                .enable_rx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("drop")
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("ethernet_stage")
        .build()
        .expect("Pipeline build should succeed");

    // Test packet that should go through all stages
    let packet = NetworkPacket::new(vec![0x08, 0x45, 0x00, 0x00, 0x28]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());

    // Test packet that should be dropped at ethernet stage
    let packet_drop = NetworkPacket::new(vec![0xFF, 0x45, 0x00, 0x00, 0x28]);
    let result = pipeline.process_receive(packet_drop, None);
    assert!(result.is_ok());
}
