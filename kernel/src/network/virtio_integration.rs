//! VirtIO-net Device Integration
//!
//! This module provides VirtIO-net specific integration with the common
//! network management layer. It wraps VirtIO-net devices and implements
//! the NetworkInterface trait for integration with common network manager.
//!
//! This is a device driver layer - all common network functionality
//! (ARP, routing, interface management) is handled by network::manager.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::device::network::{DevicePacket, MacAddress, NetworkDevice};
use crate::drivers::network::virtio_net::VirtioNetDevice;
use crate::network::ethernet::EthernetLayer;
use crate::network::ipv4::Ipv4Address;
use crate::network::{InterfaceStats, NetworkConfig, NetworkInterface, get_network_manager};

/// VirtIO Network Interface
///
/// Wraps a VirtIO-net device and implements NetworkInterface trait
/// for integration with the common network manager.
pub struct VirtIONetworkInterface {
    /// Interface name
    name: String,
    /// Underlying VirtIO-net device
    device: Arc<VirtioNetDevice>,
    /// Ethernet layer for this interface
    ethernet_layer: Arc<EthernetLayer>,
    /// IP address (if configured)
    ip_address: Mutex<Option<Ipv4Address>>,
    /// Statistics
    stats: Mutex<InterfaceStats>,
}

impl VirtIONetworkInterface {
    /// Create a new VirtIO network interface
    ///
    /// # Arguments
    ///
    /// * `name` - Interface name (e.g., "eth0")
    /// * `mmio_addr` - MMIO base address for VirtIO device
    pub fn new(name: &str, mmio_addr: usize) -> Self {
        let device = Arc::new(VirtioNetDevice::new(mmio_addr));
        let mac_address = device.get_mac_address().unwrap_or(MacAddress::new([0; 6]));
        let ethernet_layer = EthernetLayer::new(mac_address);

        Self {
            name: String::from(name),
            device,
            ethernet_layer,
            ip_address: Mutex::new(None),
            stats: Mutex::new(InterfaceStats::default()),
        }
    }

    /// Get the underlying VirtIO device
    pub fn device(&self) -> &Arc<VirtioNetDevice> {
        &self.device
    }

    /// Get the Ethernet layer
    pub fn ethernet_layer(&self) -> &Arc<EthernetLayer> {
        &self.ethernet_layer
    }
}

impl NetworkInterface for VirtIONetworkInterface {
    fn name(&self) -> &str {
        &self.name
    }

    fn mac_address(&self) -> MacAddress {
        self.device
            .get_mac_address()
            .unwrap_or(MacAddress::new([0; 6]))
    }

    fn ip_address(&self) -> Option<Ipv4Address> {
        *self.ip_address.lock()
    }

    fn set_ip_address(&self, ip: Ipv4Address) {
        *self.ip_address.lock() = Some(ip);
    }

    fn send(&self, packet: DevicePacket) -> Result<(), &'static str> {
        let mut stats = self.stats.lock();
        stats.tx_packets += 1;
        stats.tx_bytes += packet.len as u64;

        // Send through device
        self.device.send_packet(packet)
    }

    fn poll(&self) -> Result<Vec<DevicePacket>, &'static str> {
        let packets = self.device.receive_packets()?;

        // Update statistics
        let mut stats = self.stats.lock();
        stats.rx_packets += packets.len() as u64;
        stats.rx_bytes += packets.iter().map(|p| p.len as u64).sum::<u64>();

        // Process packets through Ethernet layer
        for packet in &packets {
            if let Err(e) = crate::network::protocol_stack::NetworkLayer::receive(
                self.ethernet_layer.as_ref(),
                &packet.data[..packet.len],
            ) {
                crate::println!("[VirtIO-net] Packet processing error: {:?}", e);
            }
        }

        Ok(packets)
    }

    fn stats(&self) -> InterfaceStats {
        self.stats.lock().clone()
    }
}

/// Initialize VirtIO-net interfaces
///
/// Registers VirtIO-net devices at known MMIO addresses with the network manager.
/// This should be called during system initialization.
pub fn init_virtio_net_interfaces() {
    crate::println!("[VirtIO-net] Initializing VirtIO-net interfaces...");

    let manager = get_network_manager();

    // VirtIO-net devices at known MMIO addresses (QEMU configuration)
    let devices = [
        (0x10003400usize, "eth0"), // virtio-mmio-bus.2
        (0x10003600usize, "eth1"), // virtio-mmio-bus.3
        (0x10003800usize, "eth2"), // virtio-mmio-bus.4
    ];

    for (mmio_addr, name) in devices.iter() {
        crate::println!("[VirtIO-net] Registering {} at MMIO {:#x}", name, mmio_addr);

        let interface = Arc::new(VirtIONetworkInterface::new(name, *mmio_addr));

        match manager.register_interface(name, interface.clone()) {
            Ok(_) => {
                let mac = interface.mac_address();
                crate::println!(
                    "[VirtIO-net] {} registered: MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                    name,
                    mac.as_bytes()[0],
                    mac.as_bytes()[1],
                    mac.as_bytes()[2],
                    mac.as_bytes()[3],
                    mac.as_bytes()[4],
                    mac.as_bytes()[5]
                );
            }
            Err(e) => {
                crate::println!("[VirtIO-net] Failed to register {}: {}", name, e);
            }
        }
    }

    // Set eth0 as default if registered
    if manager.get_interface("eth0").is_some() {
        manager.set_default_interface("eth0");
        crate::println!("[VirtIO-net] Set eth0 as default interface");
    }

    // Configure default gateway for QEMU user-mode networking
    let gateway = Ipv4Address::new(10, 0, 2, 2);
    manager.set_default_gateway(gateway);
    crate::println!(
        "[VirtIO-net] Set default gateway to {}.{}.{}.{}",
        gateway.as_bytes()[0],
        gateway.as_bytes()[1],
        gateway.as_bytes()[2],
        gateway.as_bytes()[3]
    );

    // Start network polling
    manager.start_polling();
    crate::println!("[VirtIO-net] Started network polling");

    crate::println!("[VirtIO-net] Initialization complete");
}

/// Legacy function - now delegates to init_virtio_net_interfaces
#[deprecated(note = "Use init_virtio_net_interfaces instead")]
pub fn init_network_stack_with_virtio() {
    init_virtio_net_interfaces();
}

/// Network communication test suite
///
/// Provides tests for VirtIO-net integration.
#[cfg(test)]
pub mod tests {
    use super::*;

    #[test_case]
    fn test_virtio_interface_creation() {
        let interface = VirtIONetworkInterface::new("test0", 0x10003000);
        assert_eq!(interface.name(), "test0");
        // Cannot test further without actual hardware
    }
}
