//!
//! Network configuration helpers.
//!
//! Applies IP/gateway/netmask settings and handles deferred IP assignment
//! until an interface is registered.

use crate::sync::IrqSpinLock;
use alloc::string::String;

use crate::network::get_network_manager;
use crate::network::ipv4::{Ipv4Address, Ipv4AddressInfo};

#[derive(Debug, Default)]
struct PendingNetworkConfig {
    ip: Option<Ipv4Address>,
    netmask: Option<Ipv4Address>,
    gateway: Option<Ipv4Address>,
    iface: Option<String>,
}

static PENDING_CONFIG: IrqSpinLock<PendingNetworkConfig> = IrqSpinLock::new(PendingNetworkConfig {
    ip: None,
    netmask: None,
    gateway: None,
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

/// Apply static network configuration from the kernel command line.
///
/// # Arguments
///
/// * `cmdline` - Boot command line containing `net.ip`, `net.mask`,
///   `net.gw`, and optional `net.iface` keys.
///
/// # Returns
///
/// This function does not return a value. Configuration is deferred when the
/// selected interface has not registered yet.
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
    if let Some(mask) = mask {
        pending.netmask = Some(mask);
    }
    if let Some(gw) = gw {
        pending.gateway = Some(gw);
    }

    let network_manager = get_network_manager();
    let ready_interface = pending
        .iface
        .as_deref()
        .and_then(|name| network_manager.get_interface(name))
        .or_else(|| {
            if pending.iface.is_none() {
                network_manager.get_default_interface()
            } else {
                None
            }
        })
        .map(|interface| String::from(interface.name()));
    drop(pending);

    if let Some(interface) = ready_interface {
        apply_pending_ip_for_interface(&interface);
    }
}

/// Set an interface IPv4 address with a compatibility `/24` netmask.
///
/// # Arguments
///
/// * `name` - Interface name to configure.
/// * `ip` - Primary IPv4 address.
///
/// # Returns
///
/// `Ok(())` after the address is installed or deferred, otherwise an error.
pub fn set_interface_ip(name: &str, ip: Ipv4Address) -> Result<(), &'static str> {
    set_interface_ip_with_mask(name, ip, Ipv4Address::new(255, 255, 255, 0))
}

/// Set an IP address and netmask on a network interface.
///
/// # Arguments
///
/// * `name` - Interface name to configure.
/// * `ip` - Primary IPv4 address.
/// * `netmask` - Contiguous IPv4 netmask.
///
/// # Returns
///
/// `Ok(())` after the address is installed or deferred, otherwise an error.
pub fn set_interface_ip_with_mask(
    name: &str,
    ip: Ipv4Address,
    netmask: Ipv4Address,
) -> Result<(), &'static str> {
    if !is_valid_netmask(netmask) {
        return Err("Invalid IPv4 netmask");
    }

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

                ipv4.set_primary_address(name, addr_info);

                if network_manager.default_interface_name().as_deref() == Some(name) {
                    let mut config = network_manager.get_config();
                    config.subnet_mask = netmask;
                    network_manager.set_config(config);
                }

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

fn is_valid_netmask(mask: Ipv4Address) -> bool {
    let bits = u32::from_be_bytes(mask.0);
    bits.leading_ones() + bits.trailing_zeros() == u32::BITS
}

/// Configure the IPv4 address and default route of one interface together.
///
/// # Arguments
///
/// * `name` - Interface name to configure.
/// * `ip` - Primary IPv4 address.
/// * `netmask` - Contiguous IPv4 network mask.
/// * `gateway` - Optional default gateway for this interface.
/// * `metric` - Default route metric; lower values are preferred.
/// * `make_default` - Whether unbound sockets should prefer this interface.
///
/// # Returns
///
/// `Ok(())` when the complete configuration is installed, otherwise an error.
pub fn configure_interface_ipv4(
    name: &str,
    ip: Ipv4Address,
    netmask: Ipv4Address,
    gateway: Option<Ipv4Address>,
    metric: u32,
    make_default: bool,
) -> Result<(), &'static str> {
    set_interface_ip_with_mask(name, ip, netmask)?;

    let network_manager = get_network_manager();
    if make_default {
        network_manager.set_default_interface(name);
    }
    network_manager.set_default_gateway_for_interface(name, gateway, metric)?;

    if network_manager.default_interface_name().as_deref() == Some(name) {
        let mut config = network_manager.get_config();
        config.subnet_mask = netmask;
        config.default_gateway = gateway;
        config.gateway_mac = None;
        network_manager.set_config(config);
    }
    Ok(())
}

/// Remove all IPv4 address and route state from an interface.
///
/// If the cleared interface was the default, the lowest-metric remaining
/// default route becomes the preferred interface.
///
/// # Arguments
///
/// * `name` - Registered interface name to clear.
///
/// # Returns
///
/// `Ok(())` after the address and routes are removed, otherwise an error.
pub fn clear_interface_ipv4(name: &str) -> Result<(), &'static str> {
    let network_manager = get_network_manager();
    let interface = network_manager
        .get_interface(name)
        .ok_or("Network interface not found")?;
    let ip_layer = network_manager
        .get_layer("ip")
        .ok_or("IPv4 layer not initialized")?;
    let ipv4 = ip_layer
        .as_any()
        .downcast_ref::<crate::network::ipv4::Ipv4Layer>()
        .ok_or("IPv4 layer type mismatch")?;

    interface.clear_ip_address();
    ipv4.remove_interface(name);

    if network_manager.default_interface_name().as_deref() == Some(name) {
        let replacement = ipv4
            .preferred_default_route()
            .map(|route| route.interface)
            .or_else(|| ipv4.first_configured_interface());
        if let Some(interface) = replacement.as_deref() {
            network_manager.set_default_interface(interface);
        }
        let mut config = network_manager.get_config();
        config.subnet_mask = replacement
            .as_deref()
            .and_then(|interface| ipv4.get_primary_address_info(interface))
            .map_or(Ipv4Address::new(0, 0, 0, 0), |info| info.netmask);
        config.default_gateway = replacement
            .as_deref()
            .and_then(|interface| ipv4.get_default_route(interface))
            .and_then(|route| route.gateway);
        config.gateway_mac = None;
        network_manager.set_config(config);
    }
    Ok(())
}

/// Apply deferred boot configuration when an interface registers.
///
/// # Arguments
///
/// * `name` - Newly registered interface name.
///
/// # Returns
///
/// This function does not return a value.
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
    let gateway = pending.gateway.take();
    pending.iface = None;
    drop(pending);

    if let Some(ip) = ip {
        let make_default = gateway.is_some()
            || get_network_manager().default_interface_name().as_deref() == Some(name);
        let _ = configure_interface_ipv4(name, ip, netmask, gateway, 100, make_default);
    }
}
