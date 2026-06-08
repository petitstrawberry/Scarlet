//!
//! Network configuration helpers.
//!
//! Applies IP/gateway/netmask settings and handles deferred IP assignment
//! until an interface is registered.

use alloc::string::String;
use spin::Mutex;

use crate::network::ipv4::{Ipv4Address, Ipv4AddressInfo};
use crate::network::{NetworkManager, get_network_manager};

#[derive(Debug, Default)]
struct PendingNetworkConfig {
    ip: Option<Ipv4Address>,
    netmask: Option<Ipv4Address>,
    iface: Option<String>,
}

static PENDING_CONFIG: Mutex<PendingNetworkConfig> = Mutex::new(PendingNetworkConfig {
    ip: None,
    netmask: None,
    iface: None,
});

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
    network_manager.set_config(config);
    if let Some(gw) = gw {
        network_manager.set_default_gateway(gw);
    }

    apply_pending_ip(network_manager, &mut pending);
}

pub fn set_interface_ip(name: &str, ip: Ipv4Address) -> Result<(), &'static str> {
    set_interface_ip_with_mask(name, ip, Ipv4Address::new(255, 255, 255, 0))
}

/// Set IP address with netmask on a network interface
pub fn set_interface_ip_with_mask(
    name: &str,
    ip: Ipv4Address,
    netmask: Ipv4Address,
) -> Result<(), &'static str> {
    let network_manager = get_network_manager();

    // Ensure network stack is initialized (NetworkManager::init sets up all layers)
    if crate::network::protocol_stack::get_network_manager()
        .get_layer("ip")
        .is_none()
    {
        // Layers are already initialized by NetworkManager::init()
        // If not present, something is wrong with initialization order
        return Err("Network stack not initialized");
    }

    if let Some(interface) = network_manager.get_interface(name) {
        // Set IP on the interface object (for backward compatibility)
        interface.set_ip_address(ip);

        // Add address to Ipv4Layer
        if let Some(ip_layer) =
            crate::network::protocol_stack::get_network_manager().get_layer("ip")
        {
            if let Some(ipv4) = ip_layer
                .as_any()
                .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
            {
                // Calculate broadcast address from IP and netmask
                let broadcast = Ipv4Address::new(
                    ip.0[0] | !netmask.0[0],
                    ip.0[1] | !netmask.0[1],
                    ip.0[2] | !netmask.0[2],
                    ip.0[3] | !netmask.0[3],
                );

                let addr_info = Ipv4AddressInfo {
                    address: ip,
                    netmask,
                    broadcast: Some(broadcast),
                    is_primary: true,
                };

                // Remove old addresses and add new one
                // (For now, just add - in future could track and replace)
                ipv4.add_address(name, addr_info);

                crate::println!(
                    "[network] {} IP set to {}.{}.{}.{}/{}",
                    name,
                    ip.0[0],
                    ip.0[1],
                    ip.0[2],
                    ip.0[3],
                    netmask_to_prefix(netmask)
                );
            } else {
                crate::println!("[network] set {} IP failed: no IPv4 layer", name);
            }
        } else {
            crate::println!("[network] set {} IP failed: no IP layer", name);
        }
        return Ok(());
    }

    // Interface not ready yet, save for later
    let mut pending = PENDING_CONFIG.lock();
    pending.ip = Some(ip);
    pending.netmask = Some(netmask);
    pending.iface = Some(String::from(name));
    Ok(())
}

/// Convert netmask to CIDR prefix length
fn netmask_to_prefix(mask: Ipv4Address) -> u8 {
    let bits = u32::from_be_bytes(mask.0);
    bits.count_ones() as u8
}

pub fn apply_pending_ip_for_interface(name: &str) {
    let mut pending = PENDING_CONFIG.lock();

    if pending.ip.is_none() {
        return;
    }

    if let Some(iface) = pending.iface.as_deref() {
        if iface != name {
            return;
        }
    }

    let ip = pending.ip.take();
    let netmask = pending
        .netmask
        .take()
        .unwrap_or(Ipv4Address::new(255, 255, 255, 0));
    pending.iface = None;
    drop(pending);

    if let Some(ip) = ip {
        let _ = set_interface_ip_with_mask(name, ip, netmask);
    }
}
