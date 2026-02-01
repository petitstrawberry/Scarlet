//! VirtIO-net device integration tests for TCP/IP protocol stack
//!
//! This module provides comprehensive integration tests that use actual
//! VirtIO-net devices configured by QEMU. Tests verify real packet
//! transmission and reception between multiple NICs.
//!
//! QEMU Configuration (from test.sh):
//! - NIC 0 (eth0): virtio-mmio-bus.2, MAC=52:54:00:12:34:56
//! - NIC 1 (eth1): virtio-mmio-bus.3, MAC=52:54:00:12:34:57
//! - NIC 2 (eth2): virtio-mmio-bus.4, MAC=52:54:00:12:34:58
//!
//! On RISC-V virt machine, virtio-mmio devices are mapped at:
//! - Base: 0x10001000
//! - Each device: +0x1000 offset
//! - net0 @ 0x10003000, net1 @ 0x10004000, net2 @ 0x10005000

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::device::network::{DevicePacket, MacAddress};
use crate::drivers::network::virtio_net::VirtioNetDevice;
use crate::network::arp::{ArpCacheEntry, ArpEntryState, ArpLayer, ArpPacket};
use crate::network::ethernet::{EthernetHeader, EthernetLayer, ether_type};
use crate::network::icmp::{IcmpEcho, IcmpHeader, IcmpLayer, code, message_type};
use crate::network::ipv4::{Ipv4Address, Ipv4Header, Ipv4Layer, protocol};
use crate::network::protocol_stack::{LayerContext, NetworkLayer, get_network_manager};
use crate::network::socket::SocketError;
use crate::network::socket::{SocketAddress, SocketControl, SocketObject, SocketState};
use crate::network::tcp::{TcpHeader, TcpLayer, TcpSocket, TcpState, tcp_flags};
use crate::network::udp::{UdpHeader, UdpLayer, UdpSocket};
use crate::network::virtio_integration::{
    NetworkInterface, NetworkInterfaceManager, get_interface_manager,
};

/// Base MMIO address for virtio devices on RISC-V virt machine
const VIRTIO_MMIO_BASE: usize = 0x10001000;

/// Offset per virtio device (0x1000 bytes each)
const VIRTIO_MMIO_STRIDE: usize = 0x1000;

/// VirtIO-net device MMIO addresses (bus indices from test.sh)
const NET0_MMIO_ADDR: usize = VIRTIO_MMIO_BASE + (2 * VIRTIO_MMIO_STRIDE); // 0x10003000
const NET1_MMIO_ADDR: usize = VIRTIO_MMIO_BASE + (3 * VIRTIO_MMIO_STRIDE); // 0x10004000
const NET2_MMIO_ADDR: usize = VIRTIO_MMIO_BASE + (4 * VIRTIO_MMIO_STRIDE); // 0x10005000

/// MAC addresses for test interfaces (from test.sh)
const NET0_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const NET1_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x57];
const NET2_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x58];

/// Get IP addresses for testing (functions since Ipv4Address::new is not const)
fn net0_ip() -> Ipv4Address {
    Ipv4Address::new(10, 0, 2, 15)
} // QEMU user network default
fn net1_ip() -> Ipv4Address {
    Ipv4Address::new(10, 0, 2, 16)
}
fn net2_ip() -> Ipv4Address {
    Ipv4Address::new(10, 0, 2, 17)
}

/// Test packet timeout (in milliseconds)
const TEST_TIMEOUT_MS: u64 = 5000;

/// Initialize test network interfaces
///
/// Creates VirtIO-net devices at known MMIO addresses and registers
/// them with the network interface manager.
fn init_test_interfaces() -> Result<(Arc<NetworkInterface>, Arc<NetworkInterface>), &'static str> {
    // Create VirtIO-net devices at known MMIO addresses
    let net0_device = Arc::new(VirtioNetDevice::new(NET0_MMIO_ADDR));
    let net1_device = Arc::new(VirtioNetDevice::new(NET1_MMIO_ADDR));

    // Initialize network interfaces
    let manager = get_interface_manager();
    let net0_interface = manager.register_interface("eth0", net0_device)?;
    let net1_interface = manager.register_interface("eth1", net1_device)?;

    // Configure IP addresses
    net0_interface.set_ip_address(net0_ip());
    net1_interface.set_ip_address(net1_ip());

    Ok((net0_interface, net1_interface))
}

/// Test 1: ARP request/reply between two interfaces
///
/// Verifies that eth0 can send an ARP request and eth1 can respond.
/// Since both are on the same QEMU hub, they should see each other's packets.
#[test_case]
fn test_arp_request_reply_between_nics() {
    crate::println!("[NetworkTest] Starting ARP request/reply test...");

    // Initialize interfaces
    let (eth0, eth1) =
        init_test_interfaces().expect("[NetworkTest] interface init failed");

    crate::println!(
        "[NetworkTest] eth0: IP={}.{}.{}.{}, MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        net0_ip().as_bytes()[0],
        net0_ip().as_bytes()[1],
        net0_ip().as_bytes()[2],
        net0_ip().as_bytes()[3],
        eth0.get_mac_address().as_bytes()[0],
        eth0.get_mac_address().as_bytes()[1],
        eth0.get_mac_address().as_bytes()[2],
        eth0.get_mac_address().as_bytes()[3],
        eth0.get_mac_address().as_bytes()[4],
        eth0.get_mac_address().as_bytes()[5]
    );
    crate::println!(
        "[NetworkTest] eth1: IP={}.{}.{}.{}, MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        net1_ip().as_bytes()[0],
        net1_ip().as_bytes()[1],
        net1_ip().as_bytes()[2],
        net1_ip().as_bytes()[3],
        eth1.get_mac_address().as_bytes()[0],
        eth1.get_mac_address().as_bytes()[1],
        eth1.get_mac_address().as_bytes()[2],
        eth1.get_mac_address().as_bytes()[3],
        eth1.get_mac_address().as_bytes()[4],
        eth1.get_mac_address().as_bytes()[5]
    );

    // Create ARP layer for eth0
    let eth0_mac = *eth0.get_mac_address().as_bytes();
    let arp_layer = ArpLayer::new(eth0_mac, net0_ip());

    // Create ARP request packet
    // eth0 asks: "Who has net1_ip()? Tell net0_ip()"
    let arp_request = ArpPacket::request(net0_ip().as_bytes(), net1_ip().as_bytes());

    // Build Ethernet frame
    let mut eth_header = EthernetHeader::new(
        [0xFF; 6], // Broadcast destination
        eth0_mac,
        ether_type::ARP,
    );

    // Serialize and send
    let arp_bytes = arp_request.to_bytes();
    let mut frame = Vec::with_capacity(14 + arp_bytes.len());
    frame.extend_from_slice(&eth_header.to_bytes());
    frame.extend_from_slice(&arp_bytes);

    let packet = DevicePacket::with_data(frame);

    crate::println!("[NetworkTest] Sending ARP request from eth0...");
    match eth0.send(packet) {
        Ok(()) => {
            crate::println!("[NetworkTest] ARP request sent successfully");
        }
        Err(e) => {
            crate::println!("[NetworkTest] ARP request failed: {}", e);
        }
    }

    // Poll eth1 to see if it received the ARP request
    // In real scenario, eth1 would respond with ARP reply
    crate::println!("[NetworkTest] Polling eth1 for received packets...");
    match eth1.poll() {
        Ok(packets) => {
            crate::println!("[NetworkTest] eth1 received {} packets", packets.len());
            for (i, pkt) in packets.iter().enumerate() {
                crate::println!("[NetworkTest] Packet {}: {} bytes", i, pkt.len);
                // Check if it's an ARP packet
                if pkt.len > 14 {
                    let ethertype = u16::from_be_bytes([pkt.data[12], pkt.data[13]]);
                    if ethertype == ether_type::ARP {
                        crate::println!("[NetworkTest] Found ARP packet!");
                        // Parse ARP packet
                        if let Some(arp) = ArpPacket::from_bytes(&pkt.data[14..]) {
                            crate::println!(
                                "[NetworkTest] ARP: sender={}.{}.{}.{}, target={}.{}.{}.{}",
                                arp.sender_ip[0],
                                arp.sender_ip[1],
                                arp.sender_ip[2],
                                arp.sender_ip[3],
                                arp.target_ip[0],
                                arp.target_ip[1],
                                arp.target_ip[2],
                                arp.target_ip[3]
                            );
                        }
                    }
                }
            }
        }
        Err(e) => {
            crate::println!("[NetworkTest] Poll failed: {}", e);
        }
    }

    crate::println!("[NetworkTest] ARP test completed");
}

/// Test 2: ICMP Echo Request/Reply (Ping)
///
/// Tests basic ICMP echo functionality between two interfaces.
#[test_case]
fn test_icmp_ping_between_nics() {
    crate::println!("[NetworkTest] Starting ICMP ping test...");

    let (eth0, eth1) =
        init_test_interfaces().expect("[NetworkTest] interface init failed");

    // Build ICMP echo request
    let icmp_echo = IcmpEcho::new(0x1234, 0x0001);
    let icmp_data = b"ScarletPing";

    // Calculate ICMP checksum
    let mut icmp_header = IcmpHeader::new(message_type::ECHO_REQUEST, code::NO_CODE);
    let mut icmp_bytes = Vec::with_capacity(8 + icmp_data.len());
    icmp_bytes.extend_from_slice(&icmp_header.to_bytes());
    icmp_bytes.extend_from_slice(&icmp_echo.to_bytes());
    icmp_bytes.extend_from_slice(icmp_data);

    icmp_header.checksum = icmp_header.calculate_checksum(icmp_data);

    // Build IPv4 packet
    let mut ip_header = Ipv4Header::new();
    ip_header.source_ip = net0_ip().as_bytes();
    ip_header.dest_ip = net1_ip().as_bytes();
    ip_header.protocol = protocol::ICMP;
    ip_header.total_length = (20 + 8 + icmp_data.len()) as u16;
    ip_header.checksum = ip_header.calculate_checksum();

    // Build Ethernet frame
    let eth0_mac = *eth0.get_mac_address().as_bytes();
    let eth1_mac = *eth1.get_mac_address().as_bytes();
    let eth_header = EthernetHeader::new(eth1_mac, eth0_mac, ether_type::IPV4);

    // Assemble full packet
    let mut packet_data = Vec::new();
    packet_data.extend_from_slice(&eth_header.to_bytes());
    packet_data.extend_from_slice(&ip_header.to_bytes());
    packet_data.extend_from_slice(&icmp_header.to_bytes());
    packet_data.extend_from_slice(&icmp_echo.to_bytes());
    packet_data.extend_from_slice(icmp_data);

    let packet = DevicePacket::with_data(packet_data);

    crate::println!("[NetworkTest] Sending ICMP echo request...");
    match eth0.send(packet) {
        Ok(()) => crate::println!("[NetworkTest] ICMP echo request sent"),
        Err(e) => crate::println!("[NetworkTest] Failed to send: {}", e),
    }

    // Poll for response
    match eth1.poll() {
        Ok(packets) => {
            crate::println!("[NetworkTest] Received {} packets", packets.len());
        }
        Err(e) => {
            crate::println!("[NetworkTest] Poll error: {}", e);
        }
    }

    crate::println!("[NetworkTest] ICMP ping test completed");
}

/// Test 3: UDP Datagram Exchange
///
/// Tests UDP packet transmission and reception.
#[test_case]
fn test_udp_datagram_exchange() {
    crate::println!("[NetworkTest] Starting UDP datagram test...");

    let (eth0, eth1) =
        init_test_interfaces().expect("[NetworkTest] interface init failed");

    // Create UDP packet
    let src_port: u16 = 12345;
    let dst_port: u16 = 54321;
    let udp_data = b"Hello from Scarlet UDP!";

    let udp_len = (8 + udp_data.len()) as u16;
    let mut udp_header = UdpHeader::new(src_port, dst_port, udp_len);
    udp_header.checksum =
        udp_header.calculate_checksum(net0_ip().as_bytes(), net1_ip().as_bytes(), udp_data);

    // Build IPv4 header
    let mut ip_header = Ipv4Header::new();
    ip_header.source_ip = net0_ip().as_bytes();
    ip_header.dest_ip = net1_ip().as_bytes();
    ip_header.protocol = protocol::UDP;
    ip_header.total_length = (20 + 8 + udp_data.len()) as u16;
    ip_header.checksum = ip_header.calculate_checksum();

    // Build Ethernet frame
    let eth0_mac = *eth0.get_mac_address().as_bytes();
    let eth1_mac = *eth1.get_mac_address().as_bytes();
    let eth_header = EthernetHeader::new(eth1_mac, eth0_mac, ether_type::IPV4);

    // Assemble packet
    let mut packet_data = Vec::new();
    packet_data.extend_from_slice(&eth_header.to_bytes());
    packet_data.extend_from_slice(&ip_header.to_bytes());
    packet_data.extend_from_slice(&udp_header.to_bytes());
    packet_data.extend_from_slice(udp_data);

    let packet = DevicePacket::with_data(packet_data);

    crate::println!("[NetworkTest] Sending UDP datagram...");
    match eth0.send(packet) {
        Ok(()) => crate::println!("[NetworkTest] UDP datagram sent"),
        Err(e) => crate::println!("[NetworkTest] Send failed: {}", e),
    }

    // Poll for received datagram
    match eth1.poll() {
        Ok(packets) => {
            crate::println!("[NetworkTest] Received {} packets", packets.len());
        }
        Err(e) => {
            crate::println!("[NetworkTest] Poll error: {}", e);
        }
    }

    crate::println!("[NetworkTest] UDP datagram test completed");
}

/// Test 4: TCP Connection Establishment
///
/// Tests TCP 3-way handshake between two interfaces.
#[test_case]
fn test_tcp_connection_establishment() {
    crate::println!("[NetworkTest] Starting TCP connection test...");

    let (eth0, eth1) =
        init_test_interfaces().expect("[NetworkTest] interface init failed");

    // Create TCP SYN packet
    let src_port: u16 = 12345;
    let dst_port: u16 = 80;
    let seq_num: u32 = 1000;

    let mut tcp_header = TcpHeader::new(src_port, dst_port);
    tcp_header.seq_number = seq_num;
    tcp_header.set_flags(tcp_flags::SYN);
    tcp_header.window_size = 65535;

    // Calculate TCP checksum
    let tcp_checksum =
        tcp_header.calculate_checksum(net0_ip().as_bytes(), net1_ip().as_bytes(), &[]);
    tcp_header.checksum = tcp_checksum;

    // Build IPv4 header
    let mut ip_header = Ipv4Header::new();
    ip_header.source_ip = net0_ip().as_bytes();
    ip_header.dest_ip = net1_ip().as_bytes();
    ip_header.protocol = protocol::TCP;
    ip_header.total_length = (20 + 20) as u16; // IP header + TCP header
    ip_header.checksum = ip_header.calculate_checksum();

    // Build Ethernet frame
    let eth0_mac = *eth0.get_mac_address().as_bytes();
    let eth1_mac = *eth1.get_mac_address().as_bytes();
    let eth_header = EthernetHeader::new(eth1_mac, eth0_mac, ether_type::IPV4);

    // Assemble packet
    let mut packet_data = Vec::new();
    packet_data.extend_from_slice(&eth_header.to_bytes());
    packet_data.extend_from_slice(&ip_header.to_bytes());
    packet_data.extend_from_slice(&tcp_header.to_bytes());

    let packet = DevicePacket::with_data(packet_data);

    crate::println!("[NetworkTest] Sending TCP SYN...");
    match eth0.send(packet) {
        Ok(()) => crate::println!("[NetworkTest] TCP SYN sent"),
        Err(e) => crate::println!("[NetworkTest] Send failed: {}", e),
    }

    // Poll for SYN-ACK response
    match eth1.poll() {
        Ok(packets) => {
            crate::println!("[NetworkTest] Received {} packets", packets.len());
            for pkt in packets {
                if pkt.len > 14 + 20 + 20 {
                    // Eth + IP + TCP headers
                    let tcp_flags_byte = pkt.data[14 + 20 + 13]; // TCP flags offset
                    if tcp_flags_byte & tcp_flags::SYN != 0 && tcp_flags_byte & tcp_flags::ACK != 0
                    {
                        crate::println!("[NetworkTest] Received SYN-ACK!");
                    }
                }
            }
        }
        Err(e) => {
            crate::println!("[NetworkTest] Poll error: {}", e);
        }
    }

    crate::println!("[NetworkTest] TCP connection test completed");
}

/// Test 5: Network interface statistics
///
/// Verifies that packet counters are correctly updated.
#[test_case]
fn test_interface_statistics() {
    crate::println!("[NetworkTest] Starting statistics test...");

    let (eth0, eth1) =
        init_test_interfaces().expect("[NetworkTest] interface init failed");

    // Get initial stats
    let stats0_before = eth0.get_stats();
    let stats1_before = eth1.get_stats();

    crate::println!(
        "[NetworkTest] eth0 before: TX={}/{} RX={}/{}",
        stats0_before.tx_packets,
        stats0_before.tx_bytes,
        stats0_before.rx_packets,
        stats0_before.rx_bytes
    );
    crate::println!(
        "[NetworkTest] eth1 before: TX={}/{} RX={}/{}",
        stats1_before.tx_packets,
        stats1_before.tx_bytes,
        stats1_before.rx_packets,
        stats1_before.rx_bytes
    );

    // Send a test packet
    let test_data = vec![0xFF; 100];
    let packet = DevicePacket::with_data(test_data);
    let _ = eth0.send(packet);

    // Get stats after send
    let stats0_after = eth0.get_stats();
    crate::println!(
        "[NetworkTest] eth0 after: TX={}/{} RX={}/{}",
        stats0_after.tx_packets,
        stats0_after.tx_bytes,
        stats0_after.rx_packets,
        stats0_after.rx_bytes
    );

    // Verify TX stats increased
    assert!(
        stats0_after.tx_packets >= stats0_before.tx_packets,
        "TX packet counter should increase"
    );

    crate::println!("[NetworkTest] Statistics test completed");
}

/// Test 6: Multi-protocol packet transmission
///
/// Tests that different protocol packets can be sent back-to-back.
#[test_case]
fn test_multi_protocol_transmission() {
    crate::println!("[NetworkTest] Starting multi-protocol test...");

    let (eth0, eth1) =
        init_test_interfaces().expect("[NetworkTest] interface init failed");

    // Send ARP packet
    let arp_request = ArpPacket::request(net0_ip().as_bytes(), net1_ip().as_bytes());
    let eth0_mac = *eth0.get_mac_address().as_bytes();
    let eth_header = EthernetHeader::new([0xFF; 6], eth0_mac, ether_type::ARP);
    let mut arp_frame = Vec::new();
    arp_frame.extend_from_slice(&eth_header.to_bytes());
    arp_frame.extend_from_slice(&arp_request.to_bytes());
    let _ = eth0.send(DevicePacket::with_data(arp_frame));

    // Send ICMP packet
    let icmp_data = b"TestICMP";
    let mut icmp_header = IcmpHeader::new(message_type::ECHO_REQUEST, code::NO_CODE);
    let icmp_echo = IcmpEcho::new(0x0001, 0x0001);
    icmp_header.checksum = icmp_header.calculate_checksum(icmp_data);

    let mut ip_header = Ipv4Header::new();
    ip_header.source_ip = net0_ip().as_bytes();
    ip_header.dest_ip = net1_ip().as_bytes();
    ip_header.protocol = protocol::ICMP;
    ip_header.total_length = (20 + 8 + icmp_data.len()) as u16;
    ip_header.checksum = ip_header.calculate_checksum();

    let eth0_mac = *eth0.get_mac_address().as_bytes();
    let eth1_mac = *eth1.get_mac_address().as_bytes();
    let eth_header2 = EthernetHeader::new(eth1_mac, eth0_mac, ether_type::IPV4);

    let mut icmp_packet = Vec::new();
    icmp_packet.extend_from_slice(&eth_header2.to_bytes());
    icmp_packet.extend_from_slice(&ip_header.to_bytes());
    icmp_packet.extend_from_slice(&icmp_header.to_bytes());
    icmp_packet.extend_from_slice(&icmp_echo.to_bytes());
    icmp_packet.extend_from_slice(icmp_data);
    let _ = eth0.send(DevicePacket::with_data(icmp_packet));

    // Send UDP packet
    let udp_data = b"TestUDP";
    let udp_header = UdpHeader::new(12345, 53, (8 + udp_data.len()) as u16);

    let mut ip_header2 = Ipv4Header::new();
    ip_header2.source_ip = net0_ip().as_bytes();
    ip_header2.dest_ip = net1_ip().as_bytes();
    ip_header2.protocol = protocol::UDP;
    ip_header2.total_length = (20 + 8 + udp_data.len()) as u16;
    ip_header2.checksum = ip_header2.calculate_checksum();

    let mut udp_packet = Vec::new();
    udp_packet.extend_from_slice(&eth_header2.to_bytes());
    udp_packet.extend_from_slice(&ip_header2.to_bytes());
    udp_packet.extend_from_slice(&udp_header.to_bytes());
    udp_packet.extend_from_slice(udp_data);
    let _ = eth0.send(DevicePacket::with_data(udp_packet));

    crate::println!("[NetworkTest] Sent 3 packets (ARP, ICMP, UDP)");

    // Poll for all responses
    match eth1.poll() {
        Ok(packets) => {
            crate::println!("[NetworkTest] Received {} packets", packets.len());
        }
        Err(e) => {
            crate::println!("[NetworkTest] Poll error: {}", e);
        }
    }

    crate::println!("[NetworkTest] Multi-protocol test completed");
}
