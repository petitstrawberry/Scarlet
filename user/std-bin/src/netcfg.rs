//! Runtime network configuration utility.

use scarlet_os::network::{
    Ipv4Address, NetworkInterfaceConfig, configure_interface_ipv4, list_interface_configs,
};
use std::process::ExitCode;

fn parse_ipv4(value: &str) -> Option<Ipv4Address> {
    let mut parts = [0u8; 4];
    let mut count = 0;
    for part in value.split('.') {
        if count == parts.len() {
            return None;
        }
        parts[count] = part.parse().ok()?;
        count += 1;
    }
    if count == parts.len() {
        Some(Ipv4Address(parts))
    } else {
        None
    }
}

fn parse_address(value: &str) -> Option<(Ipv4Address, Option<Ipv4Address>)> {
    if let Some((address, prefix)) = value.split_once('/') {
        let prefix = prefix.parse::<u8>().ok()?;
        if prefix > 32 {
            return None;
        }
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        Some((parse_ipv4(address)?, Some(Ipv4Address(mask.to_be_bytes()))))
    } else {
        Some((parse_ipv4(value)?, None))
    }
}

fn print_usage() {
    println!("Usage: netcfg --list");
    println!("       netcfg --iface <name> [--ip <addr/prefix>] [--mask <addr>] [--gw <addr>]");
    println!("              [--no-gateway] [--metric <value>] [--default]");
    println!();
    println!("Options:");
    println!("  --list             Show per-interface addresses and routes");
    println!("  --iface <name>     Interface to configure");
    println!("  --ip <addr/prefix> Set IPv4 address and optional CIDR prefix");
    println!("  --mask <addr>      Set a dotted-decimal netmask");
    println!("  --gw <addr>        Set this interface's default gateway");
    println!("  --no-gateway       Remove this interface's default route");
    println!("  --metric <value>   Set the default route metric");
    println!("  --default          Prefer this interface for unbound sockets");
}

fn prefix_length(netmask: [u8; 4]) -> u32 {
    u32::from_be_bytes(netmask).count_ones()
}

fn list_configuration() -> u8 {
    let interfaces = match list_interface_configs() {
        Ok(interfaces) => interfaces,
        Err(_) => {
            println!("netcfg: failed to list interfaces");
            return 1;
        }
    };
    if interfaces.is_empty() {
        println!("(no interfaces)");
        return 0;
    }

    for interface in &interfaces {
        let name = interface.interface_name().unwrap_or("(invalid)");
        if interface.is_default != 0 {
            println!("Interface: {name} (default)");
        } else {
            println!("Interface: {name}");
        }
        println!(
            "  MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            interface.mac_address[0],
            interface.mac_address[1],
            interface.mac_address[2],
            interface.mac_address[3],
            interface.mac_address[4],
            interface.mac_address[5]
        );
        if interface.ip_set != 0 {
            println!(
                "  IPv4: {}.{}.{}.{}/{}",
                interface.ip_address[0],
                interface.ip_address[1],
                interface.ip_address[2],
                interface.ip_address[3],
                prefix_length(interface.netmask)
            );
        } else {
            println!("  IPv4: (none)");
        }
        if interface.gateway_set != 0 {
            println!(
                "  Default route: via {}.{}.{}.{} metric {}",
                interface.gateway[0],
                interface.gateway[1],
                interface.gateway[2],
                interface.gateway[3],
                interface.metric
            );
        } else {
            println!("  Default route: (none)");
        }
    }
    0
}

fn find_interface(name: &str) -> Option<NetworkInterfaceConfig> {
    list_interface_configs()
        .ok()?
        .into_iter()
        .find(|interface| interface.interface_name() == Some(name))
}

fn main() -> ExitCode {
    ExitCode::from(run())
}

fn run() -> u8 {
    let arguments: Vec<String> = std::env::args().collect();
    let mut interface_name = None;
    let mut address = None;
    let mut address_mask = None;
    let mut netmask = None;
    let mut gateway: Option<Option<Ipv4Address>> = None;
    let mut metric = None;
    let mut make_default = false;
    let mut index = 1;

    while index < arguments.len() {
        match arguments[index].as_str() {
            "--list" => return list_configuration(),
            "--iface" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    println!("netcfg: --iface requires a name");
                    return 1;
                };
                interface_name = Some(value.as_str());
            }
            "--ip" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    println!("netcfg: --ip requires an address");
                    return 1;
                };
                let Some((parsed_address, parsed_mask)) = parse_address(value) else {
                    println!("netcfg: invalid IPv4 address {value}");
                    return 1;
                };
                address = Some(parsed_address);
                address_mask = parsed_mask;
            }
            "--mask" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    println!("netcfg: --mask requires an address");
                    return 1;
                };
                let Some(parsed) = parse_ipv4(value) else {
                    println!("netcfg: invalid netmask {value}");
                    return 1;
                };
                let bits = u32::from_be_bytes(parsed.0);
                if bits.leading_ones() + bits.trailing_zeros() != u32::BITS {
                    println!("netcfg: non-contiguous netmask {value}");
                    return 1;
                }
                netmask = Some(parsed);
            }
            "--gw" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    println!("netcfg: --gw requires an address");
                    return 1;
                };
                let Some(parsed) = parse_ipv4(value) else {
                    println!("netcfg: invalid gateway {value}");
                    return 1;
                };
                gateway = Some(Some(parsed));
            }
            "--no-gateway" => gateway = Some(None),
            "--metric" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    println!("netcfg: --metric requires a value");
                    return 1;
                };
                let Ok(parsed) = value.parse::<u32>() else {
                    println!("netcfg: invalid metric {value}");
                    return 1;
                };
                metric = Some(parsed);
            }
            "--default" => make_default = true,
            "--help" | "-h" => {
                print_usage();
                return 0;
            }
            unknown => {
                println!("netcfg: unknown option {unknown}");
                print_usage();
                return 1;
            }
        }
        index += 1;
    }

    let Some(interface_name) = interface_name else {
        println!("netcfg: --iface is required");
        print_usage();
        return 1;
    };
    let Some(current) = find_interface(interface_name) else {
        println!("netcfg: interface {interface_name} does not exist");
        return 1;
    };
    let address =
        address.or_else(|| (current.ip_set != 0).then_some(Ipv4Address(current.ip_address)));
    let Some(address) = address else {
        println!("netcfg: --ip is required for an unconfigured interface");
        return 1;
    };
    if let (Some(from_address), Some(explicit)) = (address_mask, netmask)
        && from_address != explicit
    {
        println!("netcfg: address prefix and --mask disagree");
        return 1;
    }
    let netmask = netmask
        .or(address_mask)
        .or_else(|| (current.ip_set != 0).then_some(Ipv4Address(current.netmask)));
    let Some(netmask) = netmask else {
        println!("netcfg: an address prefix or --mask is required");
        return 1;
    };
    let gateway = gateway
        .unwrap_or_else(|| (current.gateway_set != 0).then_some(Ipv4Address(current.gateway)));
    let metric = metric.unwrap_or(if current.gateway_set != 0 {
        current.metric
    } else {
        100
    });
    make_default |= current.is_default != 0;

    match configure_interface_ipv4(
        interface_name,
        address,
        netmask,
        gateway,
        metric,
        make_default,
    ) {
        Ok(()) => 0,
        Err(_) => {
            println!("netcfg: failed to configure {interface_name}");
            1
        }
    }
}
