//! TCP/IP protocol stack initialization
//!
//! This module provides initialization for the complete TCP/IP network stack.
//! It sets up all protocol layers (Ethernet, ARP, IPv4, ICMP, UDP, TCP)
//! and integrates them with the NetworkManager.

use alloc::sync::Arc;
use spin::Once;

use super::arp::ArpLayer;
use super::ethernet::EthernetLayer;
use super::icmp::IcmpLayer;
use super::ipv4::Ipv4Layer;
use super::protocol_stack::{get_network_manager, NetworkLayer};
use super::tcp::TcpLayer;
use super::udp::UdpLayer;

/// Initialize the complete TCP/IP network stack
///
/// This function should be called during kernel initialization to set up
/// all network protocol layers and register them with the NetworkManager.
pub fn init_tcp_ip_stack() {
    static INIT_DONE: Once = Once::new();

    INIT_DONE.call_once(|| {
        crate::println!("[Network] Initializing TCP/IP protocol stack");

        let network_manager = get_network_manager();

        // Get or create network device
        let device = match crate::device::network::get_network_device() {
            Some(dev) => dev,
            None => {
                crate::println!("[Network] Warning: No network device found");
                return;
            }
        };

        let config = device.get_interface_config().ok();
        let mac_address = config.mac_address;

        // Initialize Ethernet layer
        let ethernet_layer = EthernetLayer::new(mac_address);
        network_manager.register_layer("ethernet", ethernet_layer.clone());
        crate::println!(
            "[Network] Ethernet layer registered (MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X})",
            mac_address.0[0],
            mac_address.0[1],
            mac_address.0[2],
            mac_address.0[3],
            mac_address.0[4],
            mac_address.0[5]
        );

        // Initialize ARP layer
        let local_ip = Ipv4Address::new(192, 168, 1, 100);
        let arp_layer = ArpLayer::new(mac_address.0, local_ip);
        network_manager.register_layer("arp", arp_layer.clone());
        crate::println!("[Network] ARP layer registered (local IP: 192.168.1.100)");

        // Initialize IPv4 layer
        let ip_layer = Ipv4Layer::new(local_ip);
        network_manager.register_layer("ip", ip_layer.clone());
        crate::println!("[Network] IPv4 layer registered (local IP: 192.168.1.100)");

        // Initialize UDP layer
        let udp_layer = UdpLayer::new();
        network_manager.register_layer("udp", udp_layer.clone());
        crate::println!("[Network] UDP layer registered");

        // Initialize TCP layer
        let tcp_layer = TcpLayer::new();
        network_manager.register_layer("tcp", tcp_layer.clone());
        crate::println!("[Network] TCP layer registered");

        // Initialize ICMP layer
        let icmp_layer = IcmpLayer::new();
        network_manager.register_layer("icmp", icmp_layer.clone());
        crate::println!("[Network] ICMP layer registered");

        // Register protocol handlers (one-way: lower -> upper)
        // Ethernet -> ARP
        ethernet_layer.register_protocol(super::ethernet::ether_type::ARP, arp_layer.clone());

        // Ethernet -> IPv4
        ethernet_layer.register_protocol(super::ethernet::ether_type::IPV4, ip_layer.clone());

        // IPv4 -> UDP (protocol 17)
        ip_layer.register_protocol(17, udp_layer.clone());

        // IPv4 -> TCP (protocol 6)
        ip_layer.register_protocol(6, tcp_layer.clone());

        // IPv4 -> ICMP (protocol 1)
        ip_layer.register_protocol(1, icmp_layer.clone());

        crate::println!("[Network] TCP/IP protocol stack initialization complete");
    });
}

/// Create a TCP/IP protocol stack for INET domain
///
/// This function creates and registers a complete TCP/IP stack.
/// It should be called when creating an INET socket.
pub fn create_tcp_ip_stack(
    domain: super::socket::SocketDomain,
) -> Result<(), super::socket::SocketError> {
    if domain != super::socket::SocketDomain::Inet {
        return Err(super::socket::SocketError::InvalidAddress);
    }

    let network_manager = get_network_manager();

    // Check if layers are already initialized
    if network_manager.has_layer("ethernet")
        && network_manager.has_layer("arp")
        && network_manager.has_layer("ip")
        && network_manager.has_layer("udp")
        && network_manager.has_layer("tcp")
        && network_manager.has_layer("icmp")
    {
        crate::println!("[Network] TCP/IP stack already initialized");
        return Ok(());
    }

    // Initialize stack
    init_tcp_ip_stack();

    Ok(())
}
