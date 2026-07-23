//!
//! Ethernet network interface adapter.
//!
//! This bridges Ethernet-capable devices into the NetworkManager without
//! tying the core network stack to a specific device driver implementation.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::sync::Mutex;

use crate::device::network::{DevicePacket, EthernetDevice, MacAddress};
use crate::network::ipv4::Ipv4Address;
use crate::network::{InterfaceStats, NetworkInterface};

pub struct EthernetNetworkInterface {
    name: String,
    device: Arc<dyn EthernetDevice>,
    ip_address: Mutex<Option<Ipv4Address>>,
}

impl EthernetNetworkInterface {
    pub fn new(name: &str, device: Arc<dyn EthernetDevice>) -> Self {
        Self {
            name: String::from(name),
            device,
            ip_address: Mutex::new(None),
        }
    }
}

impl NetworkInterface for EthernetNetworkInterface {
    fn name(&self) -> &str {
        &self.name
    }

    fn mac_address(&self) -> MacAddress {
        self.device.mac_address().unwrap_or(MacAddress::new([0; 6]))
    }

    fn ip_address(&self) -> Option<Ipv4Address> {
        *self.ip_address.lock()
    }

    fn set_ip_address(&self, ip: Ipv4Address) {
        *self.ip_address.lock() = Some(ip);
    }

    fn send(&self, packet: DevicePacket) -> Result<(), &'static str> {
        self.device.send_packet(packet)
    }

    fn poll(&self) -> Result<Vec<DevicePacket>, &'static str> {
        self.device.receive_packets()
    }

    fn stats(&self) -> InterfaceStats {
        let stats = self.device.get_stats();
        InterfaceStats {
            tx_packets: stats.tx_packets,
            tx_bytes: stats.tx_bytes,
            rx_packets: stats.rx_packets,
            rx_bytes: stats.rx_bytes,
            drops: stats.dropped,
            errors: stats.rx_errors + stats.tx_errors,
        }
    }
}
