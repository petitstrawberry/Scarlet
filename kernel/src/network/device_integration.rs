//! Network device integration for NetworkManager
//!
//! This module bridges DeviceManager-registered network devices into
//! NetworkManager interfaces without assuming specific hardware backends.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::device::manager::DeviceManager;
use crate::device::network::{DevicePacket, MacAddress, NetworkDevice};
use crate::device::{Device, DeviceType};
use crate::network::ipv4::Ipv4Address;
use crate::network::{get_network_manager, InterfaceStats, NetworkInterface};

struct DeviceNetworkInterface {
    name: String,
    device: Arc<dyn Device>,
    mac: MacAddress,
    ip_address: Mutex<Option<Ipv4Address>>,
}

impl DeviceNetworkInterface {
    fn new(name: String, device: Arc<dyn Device>, mac: MacAddress) -> Self {
        Self {
            name,
            device,
            mac,
            ip_address: Mutex::new(None),
        }
    }

    fn network_device(&self) -> Option<&dyn NetworkDevice> {
        self.device.as_network_device()
    }
}

impl NetworkInterface for DeviceNetworkInterface {
    fn name(&self) -> &str {
        &self.name
    }

    fn mac_address(&self) -> MacAddress {
        self.mac
    }

    fn ip_address(&self) -> Option<Ipv4Address> {
        *self.ip_address.lock()
    }

    fn set_ip_address(&self, ip: Ipv4Address) {
        *self.ip_address.lock() = Some(ip);
    }

    fn send(&self, packet: DevicePacket) -> Result<(), &'static str> {
        let device = self
            .network_device()
            .ok_or("Device is not a network device")?;
        device.send_packet(packet)
    }

    fn poll(&self) -> Result<Vec<DevicePacket>, &'static str> {
        let device = self
            .network_device()
            .ok_or("Device is not a network device")?;
        device.receive_packets()
    }

    fn stats(&self) -> InterfaceStats {
        let device = match self.network_device() {
            Some(device) => device,
            None => return InterfaceStats::default(),
        };
        let stats = device.get_stats();
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

/// Register network interfaces for all discovered network devices.
pub fn init_network_interfaces_from_devices() {
    let network_manager = get_network_manager();
    let device_manager = DeviceManager::get_manager();

    for (name, device) in device_manager.get_named_devices() {
        if device.device_type() != DeviceType::Network {
            continue;
        }

        if network_manager.get_interface(&name).is_some() {
            continue;
        }

        let network_device = match device.as_network_device() {
            Some(device) => device,
            None => {
                crate::early_println!("[network] Skipping {}: not a NetworkDevice", name);
                continue;
            }
        };

        let config = match network_device.get_interface_config() {
            Ok(config) => config,
            Err(e) => {
                crate::early_println!("[network] Skipping {}: missing config ({})", name, e);
                continue;
            }
        };

        let interface = Arc::new(DeviceNetworkInterface::new(
            name.clone(),
            device.clone(),
            config.mac_address,
        ));

        if let Err(e) = network_manager.register_interface(&name, interface) {
            crate::early_println!("[network] Failed to register {}: {}", name, e);
        } else {
            crate::early_println!("[network] Registered interface {}", name);
        }
    }
}

#[cfg(feature = "network")]
crate::late_initcall!(init_network_interfaces_from_devices);
