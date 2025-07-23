//! Demonstration of the beautiful Phase 2 network protocol API

use crate::network::*;

/// Example demonstrating the beautiful pipeline construction API
pub fn demo_beautiful_api() {
    // Build a complete network receive pipeline with type-safe routing
    let _receive_pipeline = FlexiblePipeline::builder()
        .add_stage(
            EthernetStage::builder()
                .route_to(EtherType::IPv4, "ipv4")       // Type-safe enum
                .route_to(EtherType::ARP, "arp")         // Type-safe enum
                .route_to(EtherType::IPv6, "ipv6")       // Type-safe enum
                .add_ethertype_route(0x8100, "vlan")     // Raw value for custom protocols
                .enable_rx()
                .with_src_mac([0x02, 0x00, 0x00, 0x00, 0x00, 0x01])
                .build()
        )
        .add_stage(
            IPv4Stage::builder()
                .route_to(IpProtocol::TCP, "tcp")        // Type-safe enum
                .route_to(IpProtocol::UDP, "udp")        // Type-safe enum
                .route_to(IpProtocol::ICMP, "icmp")      // Type-safe enum
                .add_protocol_route(89, "ospf")          // Raw value for custom protocols
                .enable_rx()
                .with_default_ttl(64)
                .build()
        )
        .add_stage(ArpStage::builder().enable_rx().build())
        .set_default_rx_entry("ethernet")
        .build()
        .expect("Pipeline build should succeed");

    // Build a complete network transmit pipeline
    let _transmit_pipeline = FlexiblePipeline::builder()
        .add_stage(
            IPv4Stage::builder()
                .with_default_ttl(128)
                .enable_tx()
                .build()
        )
        .add_stage(
            EthernetStage::builder()
                .with_src_mac([0x02, 0x00, 0x00, 0x00, 0x00, 0x01])
                .enable_tx()
                .build()
        )
        .add_stage(ArpStage::builder().enable_tx().build())
        .set_default_tx_entry("ipv4")
        .build()
        .expect("Pipeline build should succeed");

    // Advanced configuration with custom MAC and multiple routing
    let _advanced_stage = EthernetStage::builder()
        .with_stage_id("eth0")  // Custom stage name
        .with_src_mac([0x02, 0x00, 0x00, 0x00, 0x00, 0x01])  // Custom MAC
        .add_enum_routes(&[
            (EtherType::IPv4, "ipv4"),
            (EtherType::IPv6, "ipv6"),
            (EtherType::ARP, "arp"),
        ])
        .add_routes(&[
            (0x8863, "pppoe_discovery"),  // Custom protocols
            (0x8864, "pppoe_session"),
        ])
        .enable_both()  // Enable both Tx/Rx
        .build();
}

/// Demonstrate type-safe protocol constants
pub fn demo_type_safe_protocols() {
    // EtherType constants are type-safe
    assert_eq!(EtherType::IPv4.as_u16(), 0x0800);
    assert_eq!(EtherType::ARP.as_u16(), 0x0806);
    assert_eq!(EtherType::IPv6.as_u16(), 0x86DD);
    
    // Conversion from raw values
    assert_eq!(EtherType::from_u16(0x0800), Some(EtherType::IPv4));
    assert_eq!(EtherType::from_u16(0x9999), None);
    
    // Human readable names
    assert_eq!(EtherType::IPv4.name(), "IPv4");
    assert_eq!(EtherType::ARP.name(), "ARP");

    // IP Protocol constants are type-safe
    assert_eq!(IpProtocol::TCP.as_u8(), 6);
    assert_eq!(IpProtocol::UDP.as_u8(), 17);
    assert_eq!(IpProtocol::ICMP.as_u8(), 1);
    
    // Conversion from raw values
    assert_eq!(IpProtocol::from_u8(6), Some(IpProtocol::TCP));
    assert_eq!(IpProtocol::from_u8(99), None);
    
    // Human readable names
    assert_eq!(IpProtocol::TCP.name(), "TCP");
    assert_eq!(IpProtocol::UDP.name(), "UDP");
}

/// Demonstrate O(1) HashMap routing
pub fn demo_o1_routing() {
    // O(1) EtherType routing - no loops!
    let ethernet_matcher = EtherTypeToStage::new()
        .add_mapping(0x0800, "ipv4")
        .add_mapping(0x0806, "arp")
        .add_mapping(0x86DD, "ipv6");
    
    // Instant routing lookup
    assert_eq!(ethernet_matcher.get_next_stage(0x0800).unwrap(), "ipv4");
    assert_eq!(ethernet_matcher.get_next_stage(0x0806).unwrap(), "arp");
    assert_eq!(ethernet_matcher.get_next_stage(0x86DD).unwrap(), "ipv6");

    // O(1) IP Protocol routing - no loops!
    let ip_matcher = IpProtocolToStage::new()
        .add_mapping(6, "tcp")
        .add_mapping(17, "udp")
        .add_mapping(1, "icmp");
    
    // Instant routing lookup
    assert_eq!(ip_matcher.get_next_stage(6).unwrap(), "tcp");
    assert_eq!(ip_matcher.get_next_stage(17).unwrap(), "udp");
    assert_eq!(ip_matcher.get_next_stage(1).unwrap(), "icmp");
}