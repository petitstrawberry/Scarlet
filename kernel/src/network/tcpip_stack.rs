//! TCP/IP protocol stack initialization
//!
//! This module provides initialization for complete TCP/IP network stack.
//! It sets up all protocol layers (Ethernet, ARP, IPv4, ICMP, UDP, TCP)
//! and integrates them with NetworkManager.

use alloc::sync::Arc;
use spin::Once;

use super::arp::ArpLayer;
use super::ethernet::EthernetLayer;
use super::icmp::IcmpLayer;
use super::ipv4::Ipv4Layer;
use super::protocol_stack::get_network_manager;
use super::tcp::TcpLayer;
use super::udp::UdpLayer;

/// Initialize complete TCP/IP network stack
///
/// This function should be called during kernel initialization to set up
/// all network protocol layers and register them with NetworkManager.
pub fn init_tcp_ip_stack() {
    static INIT_DONE: Once = Once::new();

    INIT_DONE.call_once(|| {
        let network_manager = get_network_manager();

        let device = match crate::device::network::get_network_device() {
            Some(dev) => dev,
            None => {
                return;
            }
        };

        let config = device.get_interface_config().ok();
        let mac_address = config.mac_address;

        let ethernet_layer = EthernetLayer::new(mac_address);
        network_manager.register_layer("ethernet", ethernet_layer.clone());

        let local_ip = Ipv4Address::new(192, 168, 1, 100);
        let arp_layer = ArpLayer::new(mac_address.as_bytes().try_into().unwrap(), local_ip);
        network_manager.register_layer("arp", arp_layer.clone());

        let ip_layer = Ipv4Layer::new(local_ip);
        network_manager.register_layer("ip", ip_layer.clone());

        let udp_layer = UdpLayer::new();
        network_manager.register_layer("udp", udp_layer.clone());

        let tcp_layer = TcpLayer::new();
        network_manager.register_layer("tcp", tcp_layer.clone());

        let icmp_layer = IcmpLayer::new();
        network_manager.register_layer("icmp", icmp_layer.clone());

        ethernet_layer.register_protocol(super::ethernet::ether_type::ARP, arp_layer.clone());
        ethernet_layer.register_protocol(super::ethernet::ether_type::IPV4, ip_layer.clone());

        ip_layer.register_protocol(17, udp_layer.clone());
        ip_layer.register_protocol(6, tcp_layer.clone());
        ip_layer.register_protocol(1, icmp_layer.clone());
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

    if network_manager.has_layer("ethernet")
        && network_manager.has_layer("arp")
        && network_manager.has_layer("ip")
        && network_manager.has_layer("udp")
        && network_manager.has_layer("tcp")
        && network_manager.has_layer("icmp")
    {
        return Ok(());
    }

    init_tcp_ip_stack();

    Ok(())
}
