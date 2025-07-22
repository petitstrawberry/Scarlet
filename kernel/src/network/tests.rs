#![cfg(test)]

use crate::network::*;
use crate::network::test_helpers::*;
use alloc::string::ToString;
use alloc::{vec, format};

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
fn test_protocol_header_parsing() {
    // Test protocol header parsing
    let header = TestProtocolHeader::new(0x01);
    let bytes = header.to_bytes();
    assert_eq!(bytes, vec![0x01]);
    
    let parsed = TestProtocolHeader::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.next_type, 0x01);
    
    // Test insufficient data
    let result = TestProtocolHeader::from_bytes(&[]);
    assert!(result.is_err());
}

#[test_case]
fn test_pipeline_routing_with_protocol() {
    use alloc::collections::BTreeMap;
    
    // Create custom routing table
    let mut routes = BTreeMap::new();
    routes.insert(0x01, "stage_a".to_string());
    routes.insert(0x02, "stage_b".to_string());
    
    // Build pipeline with protocol parser and target stages
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestProtocolStageBuilder::new("parser")
                .as_protocol_parser()
                .with_custom_routes(routes)
                .build_rx_stage()
        )
        .add_stage(
            TestProtocolStageBuilder::new("stage_a")
                .build_rx_stage()
        )
        .add_stage(
            TestProtocolStageBuilder::new("stage_b")
                .build_rx_stage()
        )
        .set_default_rx_entry("parser")
        .build()
        .unwrap();

    // Test routing to stage_a
    let payload = vec![0x01, 0xAA, 0xBB, 0xCC]; // Header: 0x01, Data: [0xAA, 0xBB, 0xCC]
    let packet = NetworkPacket::new(payload);
    let result = pipeline.process_receive(packet, None).unwrap();
    
    // Verify header was extracted and payload updated
    assert_eq!(result.payload(), &vec![0xAA, 0xBB, 0xCC]);
    assert_eq!(result.get_header("test_protocol"), Some(&vec![0x01]));
    assert_eq!(result.get_hint("test_protocol_type"), Some("0x01"));

    // Test routing to stage_b
    let payload = vec![0x02, 0xDD, 0xEE]; // Header: 0x02, Data: [0xDD, 0xEE]
    let packet = NetworkPacket::new(payload);
    let result = pipeline.process_receive(packet, None).unwrap();
    
    // Verify header was extracted and payload updated
    assert_eq!(result.payload(), &vec![0xDD, 0xEE]);
    assert_eq!(result.get_header("test_protocol"), Some(&vec![0x02]));
    assert_eq!(result.get_hint("test_protocol_type"), Some("0x02"));
    
    // Test unknown protocol type
    let payload = vec![0xFF, 0x11, 0x22]; // Unknown header type
    let packet = NetworkPacket::new(payload);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_err()); // Should fail due to unknown protocol type
}

#[test_case]
fn test_protocol_transmission() {
    // Build pipeline with protocol generator
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestProtocolStageBuilder::new("generator")
                .as_protocol_generator(0x05)
                .build_tx_stage()
        )
        .set_default_tx_entry("generator")
        .build()
        .unwrap();

    // Test protocol header generation
    let payload = vec![0x11, 0x22, 0x33];
    let packet = NetworkPacket::new(payload);
    let result = pipeline.process_transmit(packet, None).unwrap();
    
    // Verify header was prepended
    assert_eq!(result.payload(), &vec![0x05, 0x11, 0x22, 0x33]);
    assert_eq!(result.get_header("test_protocol"), Some(&vec![0x05]));
    assert_eq!(result.get_hint("test_protocol_type"), Some("0x05"));
}

#[test_case]
fn test_bidirectional_protocol_pipeline() {
    use alloc::collections::BTreeMap;
    
    // Create routing for receive path
    let mut rx_routes = BTreeMap::new();
    rx_routes.insert(0x10, "rx_target".to_string());
    
    // Build bidirectional pipeline
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestProtocolStageBuilder::new("protocol_layer")
                .as_protocol_parser()
                .with_custom_routes(rx_routes)
                .build_bidirectional_stage()
        )
        .add_stage(
            TestProtocolStageBuilder::new("rx_target")
                .build_rx_stage()
        )
        .set_default_rx_entry("protocol_layer")
        .set_default_tx_entry("protocol_layer")
        .build()
        .unwrap();

    // Test receive path
    let rx_payload = vec![0x10, 0x01, 0x02, 0x03];
    let rx_packet = NetworkPacket::new(rx_payload);
    let rx_result = pipeline.process_receive(rx_packet, None).unwrap();
    
    assert_eq!(rx_result.payload(), &vec![0x01, 0x02, 0x03]);
    assert_eq!(rx_result.get_hint("test_protocol_type"), Some("0x10"));

    // Test transmit path (uses echo handler in this case)
    let tx_payload = vec![0x04, 0x05, 0x06];
    let tx_packet = NetworkPacket::new(tx_payload.clone());
    let tx_result = pipeline.process_transmit(tx_packet, None).unwrap();
    
    assert_eq!(tx_result.payload(), &tx_payload); // Echo handler doesn't modify
}

#[test_case]
fn test_pipeline_routing_with_add_route() {
    use crate::network::test_helpers::TEST_PROTOCOL_TYPE_A;
    
    // Create a pipeline with routing-enabled stages
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("router")
                .add_route(TEST_PROTOCOL_TYPE_A, "type_a_handler")
                .add_route(0x02, "type_b_handler")
                .enable_rx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("type_a_handler")
                .enable_rx()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("type_b_handler")
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("router")
        .build()
        .expect("Pipeline build should succeed");

    // Test routing with protocol type A (0x01)
    let packet = NetworkPacket::new(vec![TEST_PROTOCOL_TYPE_A, 0xAA, 0xBB, 0xCC]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());

    // Test routing with protocol type B (0x02)
    let packet = NetworkPacket::new(vec![0x02, 0xDD, 0xEE, 0xFF]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());

    // Test unsupported protocol type should fail
    let packet = NetworkPacket::new(vec![0xFF, 0x11, 0x22, 0x33]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_err());
}

#[test_case]
fn test_empty_routing_fallback() {
    // Test that stages without routing rules fall back to echo behavior
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("simple_echo")
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("simple_echo")
        .build()
        .expect("Pipeline build should succeed");

    let packet = NetworkPacket::new(vec![1, 2, 3, 4]);
    let result = pipeline.process_receive(packet, None);
    
    assert!(result.is_ok());
    let processed_packet = result.unwrap();
    assert_eq!(processed_packet.payload(), &vec![1, 2, 3, 4]);
}

#[test_case]
fn test_multiple_route_additions() {
    // Test dynamic route addition
    let stage = TestStageBuilder::new("multi_router")
        .add_route(0x01, "stage1")
        .add_route(0x02, "stage2")
        .add_route(0x03, "stage3")
        .enable_rx()
        .build();
    
    assert_eq!(stage.stage_id, "multi_router");
    assert!(stage.rx_handler.is_some());
    assert!(stage.tx_handler.is_none());
}

#[test_case]
fn test_pipeline_routing_with_tracing() {
    use crate::network::test_helpers::TEST_PROTOCOL_TYPE_A;
    
    // Create a pipeline with tracing-enabled stages
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("router")
                .add_route(TEST_PROTOCOL_TYPE_A, "type_a_handler")
                .add_route(0x02, "type_b_handler")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("type_a_handler")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("type_b_handler")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .set_default_rx_entry("router")
        .build()
        .expect("Pipeline build should succeed");

    // Test routing with protocol type A (0x01)
    let packet = NetworkPacket::new(vec![TEST_PROTOCOL_TYPE_A, 0xAA, 0xBB, 0xCC]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify tracing information
    let trace = processed_packet.get_hint("pipeline_trace").unwrap_or("");
    assert_eq!(trace, "router -> type_a_handler");
    
    // Verify processing markers
    assert_eq!(processed_packet.get_hint("processed_by_router"), Some("protocol_type:0x01"));
    assert_eq!(processed_packet.get_hint("processed_by_type_a_handler"), Some("true"));
    
    // Verify packet transformation
    assert_eq!(processed_packet.payload(), &vec![0xAA, 0xBB, 0xCC]); // Header removed
    assert!(processed_packet.get_header("test_protocol").is_some()); // Header added
    assert_eq!(processed_packet.get_hint("test_protocol_type"), Some("0x01"));

    // Test routing with protocol type B (0x02)
    let packet = NetworkPacket::new(vec![0x02, 0xDD, 0xEE, 0xFF]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify tracing information for type B
    let trace = processed_packet.get_hint("pipeline_trace").unwrap_or("");
    assert_eq!(trace, "router -> type_b_handler");
    
    // Verify processing markers
    assert_eq!(processed_packet.get_hint("processed_by_router"), Some("protocol_type:0x02"));
    assert_eq!(processed_packet.get_hint("processed_by_type_b_handler"), Some("true"));
    
    // Verify packet transformation
    assert_eq!(processed_packet.payload(), &vec![0xDD, 0xEE, 0xFF]); // Header removed
    assert!(processed_packet.get_header("test_protocol").is_some()); // Header added
    assert_eq!(processed_packet.get_hint("test_protocol_type"), Some("0x02"));
}

#[test_case]
fn test_packet_state_verification() {
    // Test packet state at different stages
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("preprocessor")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .set_default_rx_entry("preprocessor")
        .build()
        .expect("Pipeline build should succeed");

    let mut packet = NetworkPacket::new(vec![1, 2, 3, 4, 5]);
    packet.add_header("original_header", vec![0xFF, 0xFE]);
    packet.set_hint("original_hint", "test_value");
    
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify original data is preserved
    assert_eq!(processed_packet.payload(), &vec![1, 2, 3, 4, 5]);
    assert_eq!(processed_packet.get_header("original_header"), Some(&vec![0xFF, 0xFE]));
    assert_eq!(processed_packet.get_hint("original_hint"), Some("test_value"));
    
    // Verify trace was added
    assert_eq!(processed_packet.get_hint("pipeline_trace"), Some("preprocessor"));
    assert_eq!(processed_packet.get_hint("processed_by_preprocessor"), Some("true"));
}

#[test_case]
fn test_complex_routing_trace() {
    use crate::network::test_helpers::{TEST_PROTOCOL_TYPE_A, TEST_PROTOCOL_TYPE_B};
    
    // Create a more complex pipeline with multiple routing stages
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("initial_router")
                .add_route(TEST_PROTOCOL_TYPE_A, "l2_processor")
                .add_route(TEST_PROTOCOL_TYPE_B, "l3_processor")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("l2_processor")
                .add_route(0x03, "final_handler")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("l3_processor")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .add_stage(
            TestStageBuilder::new("final_handler")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .set_default_rx_entry("initial_router")
        .build()
        .expect("Pipeline build should succeed");

    // Test multi-stage routing: initial_router -> l2_processor -> final_handler
    let packet = NetworkPacket::new(vec![TEST_PROTOCOL_TYPE_A, 0x03, 0x11, 0x22]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify complex trace path
    let trace = processed_packet.get_hint("pipeline_trace").unwrap_or("");
    assert_eq!(trace, "initial_router -> l2_processor -> final_handler");
    
    // Verify all processing markers exist
    assert!(processed_packet.get_hint("processed_by_initial_router").is_some());
    assert!(processed_packet.get_hint("processed_by_l2_processor").is_some());
    assert!(processed_packet.get_hint("processed_by_final_handler").is_some());
    
    // Verify final packet state - payload should have both headers removed
    assert_eq!(processed_packet.payload(), &vec![0x11, 0x22]);
}

#[test_case]
fn test_protocol_stage_builder_tracing() {
    // Test Echo handler with tracing
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestProtocolStageBuilder::new("echo_stage")
                .enable_tracing()
                .build_rx_stage()
        )
        .set_default_rx_entry("echo_stage")
        .build()
        .expect("Pipeline build should succeed");

    let packet = NetworkPacket::new(vec![1, 2, 3, 4]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify tracing was enabled
    let trace = processed_packet.get_hint("pipeline_trace").unwrap_or("");
    assert_eq!(trace, "echo_stage");
}

#[test_case]
fn test_protocol_stage_builder_protocol_parser_tracing() {
    // Create custom routing for test
    let mut routes = alloc::collections::BTreeMap::new();
    routes.insert(TEST_PROTOCOL_TYPE_A, "next_stage".to_string());
    
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestProtocolStageBuilder::new("parser_stage")
                .as_protocol_parser()
                .with_custom_routes(routes)
                .enable_tracing()
                .build_rx_stage()
        )
        .add_stage(
            TestProtocolStageBuilder::new("next_stage")
                .enable_tracing()
                .build_rx_stage()
        )
        .set_default_rx_entry("parser_stage")
        .build()
        .expect("Pipeline build should succeed");

    // Create packet with protocol header
    let packet = NetworkPacket::new(vec![TEST_PROTOCOL_TYPE_A, 0x11, 0x22, 0x33]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify tracing through both stages
    let trace = processed_packet.get_hint("pipeline_trace").unwrap_or("");
    assert_eq!(trace, "parser_stage -> next_stage");
    
    // Verify protocol parsing occurred
    assert!(processed_packet.get_hint("test_protocol_type").is_some());
    assert_eq!(processed_packet.payload(), &vec![0x11, 0x22, 0x33]);
}

#[test_case]
fn test_protocol_stage_builder_bidirectional_tracing() {
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            TestStageBuilder::new("target_stage")
                .enable_rx()
                .enable_tracing()
                .build()
        )
        .add_stage(
            TestProtocolStageBuilder::new("bidirectional_stage")
                .as_protocol_parser()
                .add_route(TEST_PROTOCOL_TYPE_B, "target_stage")
                .enable_tracing()
                .build_bidirectional_stage()
        )
        .set_default_rx_entry("bidirectional_stage")
        .build()
        .expect("Pipeline build should succeed");

    let packet = NetworkPacket::new(vec![TEST_PROTOCOL_TYPE_B, 0xAA, 0xBB]);
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify tracing was enabled for both stages
    let trace = processed_packet.get_hint("pipeline_trace").unwrap_or("");
    assert_eq!(trace, "bidirectional_stage -> target_stage");
    
    // Verify protocol parsing occurred
    assert!(processed_packet.get_hint("test_protocol_type").is_some());
    assert_eq!(processed_packet.payload(), &vec![0xAA, 0xBB]);
}

#[test_case]
fn test_flexible_pipeline_builder_typed() {
    // Test type-safe pipeline building
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            EchoStageBuilder::new()
                .build()
        )
        .add_stage(
            ProtocolStageBuilder::new()
                .build()
        )
        .set_default_rx_entry_typed::<EchoProtocol>()
        .set_default_tx_entry_typed::<EchoProtocol>()
        .build()
        .expect("Pipeline build should succeed");

    assert!(pipeline.has_stage_typed::<EchoProtocol>());
    
    // Also test the string-based versions still work
    assert!(pipeline.has_stage("echo"));
    
    let stage_ids = pipeline.stage_ids();
    assert_eq!(stage_ids.len(), 2);
    assert!(stage_ids.contains(&"echo".to_string()));
}

#[test_case]
fn test_individual_stage_builders() {
    // Test using individual stage builders instead of the generic TestStageBuilder
    let pipeline = FlexiblePipeline::builder()
        .add_stage(EchoStageBuilder::new().build())
        .add_stage(
            ProtocolStageBuilder::new()
                .add_route_typed::<EchoProtocol>(TEST_PROTOCOL_TYPE_A)
                .build()
        )
        .set_default_rx_entry_typed::<EchoProtocol>()
        .set_default_tx_entry_typed::<EchoProtocol>()
        .build()
        .expect("Pipeline build should succeed");

    assert!(pipeline.has_stage_typed::<EchoProtocol>());
}

#[test_case]
fn test_echo_stage_with_tracing() {
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            EchoStageBuilder::new()
                .enable_tracing()
                .build()
        )
        .set_default_rx_entry_typed::<EchoProtocol>()
        .build()
        .expect("Pipeline build should succeed");

    let packet = NetworkPacket::new(vec![1, 2, 3, 4]);
    let result = pipeline.process_receive(packet, None);
    
    assert!(result.is_ok());
    let processed_packet = result.unwrap();
    
    // Check if tracing was added
    assert!(processed_packet.get_hint("pipeline_trace").is_some());
}

#[test_case]
fn test_protocol_stage_routing() {
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            ProtocolStageBuilder::new()
                .add_route_typed::<EchoProtocol>(TEST_PROTOCOL_TYPE_A)
                .enable_tracing()
                .build()
        )
        .add_stage(EchoStageBuilder::new().build())
        .set_default_rx_entry("protocol")
        .build()
        .expect("Pipeline build should succeed");

    // Create packet with protocol header
    let packet = NetworkPacket::new(vec![TEST_PROTOCOL_TYPE_A, 0xAA, 0xBB, 0xCC]);
    let result = pipeline.process_receive(packet, None);
    
    assert!(result.is_ok());
    let processed_packet = result.unwrap();
    
    // Verify protocol was parsed and routed correctly
    assert_eq!(processed_packet.payload(), &vec![0xAA, 0xBB, 0xCC]);
    
    // Check tracing shows the routing path
    let trace = processed_packet.get_hint("pipeline_trace").unwrap_or("");
    assert!(trace.contains("protocol"));
}

#[test_case]
fn test_realistic_protocol_stack() {
    // Test a more realistic protocol stack
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            ProtocolStageBuilder::with_stage_id("ethernet")
                .add_route(0x08, "ip") // IPv4 packets to IP stage
                .build()
        )
        .add_stage(
            ProtocolStageBuilder::with_stage_id("ip")
                .add_route(6, "tcp")  // TCP packets to TCP stage
                .add_route(17, "udp") // UDP packets to UDP stage
                .build()
        )
        .add_stage(
            ProtocolStageBuilder::with_stage_id("tcp")
                .build()
        )
        .add_stage(
            ProtocolStageBuilder::with_stage_id("udp")
                .build()
        )
        .set_default_rx_entry("ethernet")
        .set_default_tx_entry("tcp")
        .build()
        .expect("Protocol stack build should succeed");

    // Verify all stages exist
    assert!(pipeline.has_stage("ethernet"));
    assert!(pipeline.has_stage("ip"));
    assert!(pipeline.has_stage("tcp"));
    assert!(pipeline.has_stage("udp"));
    
    // Test packet processing with proper protocol headers
    // Create a packet with test protocol header pointing to IP (0x08)
    let mut packet = NetworkPacket::new(vec![0x08, 0x06, 0x02, 0x03]); // First byte is protocol type
    packet.add_header("test_protocol", vec![0x08]); // Add test protocol header
    
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
}
