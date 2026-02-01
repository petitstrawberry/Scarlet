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
use crate::network::{InterfaceStats, NetworkInterface, NetworkManager, get_network_manager};

#[derive(Debug, Default)]
struct PendingNetworkConfig {
    ip: Option<Ipv4Address>,
    iface: Option<String>,
}

static PENDING_CONFIG: Mutex<PendingNetworkConfig> = Mutex::new(PendingNetworkConfig {
    ip: None,
    iface: None,
});

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

fn parse_ipv4(value: &str) -> Option<Ipv4Address> {
    let mut parts = [0u8; 4];
    let mut index = 0;
    for part in value.split('.') {
        if index >= parts.len() {
            return None;
        }
        parts[index] = part.parse::<u8>().ok()?;
        index += 1;
    }
    if index == parts.len() {
        Some(Ipv4Address::from_bytes(parts))
    } else {
        None
    }
}

fn apply_pending_ip(network_manager: &NetworkManager, pending: &mut PendingNetworkConfig) {
    let Some(ip) = pending.ip else {
        return;
    };

    let target = pending.iface.as_deref();
    let interface = match target {
        Some(name) => network_manager.get_interface(name),
        None => network_manager.get_default_interface(),
    };

    if let Some(interface) = interface {
        interface.set_ip_address(ip);
        pending.ip = None;
        if target.is_some() {
            pending.iface = None;
        }
    }
}

/// Apply network configuration from the kernel command line.
pub fn apply_cmdline_config(cmdline: &str) {
    let mut pending = PENDING_CONFIG.lock();
    let mut ip = None;
    let mut iface = None;
    let mut gw = None;
    let mut dns = None;
    let mut mask = None;

    for token in cmdline.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };

        match key {
            "ip" | "net.ip" => {
                if value != "dhcp" {
                    ip = parse_ipv4(value);
                }
            }
            "gw" | "net.gw" => {
                gw = parse_ipv4(value);
            }
            "dns" | "net.dns" => {
                dns = parse_ipv4(value);
            }
            "mask" | "net.mask" | "net.netmask" => {
                mask = parse_ipv4(value);
            }
            "iface" | "net.iface" => {
                iface = Some(String::from(value));
            }
            _ => {}
        }
    }

    if let Some(ip) = ip {
        pending.ip = Some(ip);
    }
    if let Some(iface) = iface {
        pending.iface = Some(iface);
    }

    let network_manager = get_network_manager();
    let mut config = network_manager.get_config();
    if let Some(mask) = mask {
        config.subnet_mask = mask;
    }
    if let Some(dns) = dns {
        config.dns_server = Some(dns);
    }
    network_manager.set_config(config);
    if let Some(gw) = gw {
        network_manager.set_default_gateway(gw);
    }

    apply_pending_ip(network_manager, &mut pending);
}

pub fn set_interface_ip(name: &str, ip: Ipv4Address) -> Result<(), &'static str> {
    let network_manager = get_network_manager();
    if let Some(interface) = network_manager.get_interface(name) {
        interface.set_ip_address(ip);
        if crate::network::protocol_stack::get_network_manager()
            .get_layer("ip")
            .is_none()
        {
            crate::network::tcpip_stack::init_tcp_ip_stack();
        }

        if let Some(ip_layer) =
            crate::network::protocol_stack::get_network_manager().get_layer("ip")
        {
            if let Some(ipv4) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            {
                crate::early_println!(
                    "[network] set {} IP to {}.{}.{}.{}",
                    name,
                    ip.0[0],
                    ip.0[1],
                    ip.0[2],
                    ip.0[3]
                );
                ipv4.set_local_ip(ip);
            } else {
                crate::early_println!("[network] set {} IP failed: no IPv4 layer", name);
            }
        } else {
            crate::early_println!("[network] set {} IP failed: no IP layer", name);
        }
        return Ok(());
    }

    let mut pending = PENDING_CONFIG.lock();
    pending.ip = Some(ip);
    pending.iface = Some(String::from(name));
    Ok(())
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

        if let Err(e) = network_manager.register_interface(&name, interface.clone()) {
            crate::early_println!("[network] Failed to register {}: {}", name, e);
        } else {
            crate::early_println!("[network] Registered interface {}", name);
            let mut pending = PENDING_CONFIG.lock();
            apply_pending_ip(network_manager, &mut pending);
        }
    }
}

#[cfg(feature = "network")]
crate::late_initcall!(init_network_interfaces_from_devices);
