#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::network::{
    Ipv4Address, list_interfaces, set_default_gateway, set_interface_ipv4, set_netmask,
};
use std::println;

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
        Some(Ipv4Address::new(parts[0], parts[1], parts[2], parts[3]))
    } else {
        None
    }
}

fn print_usage() {
    println!("Usage: netcfg --iface <name> [--ip <addr>] [--mask <addr>] [--gw <addr>]");
    println!("       netcfg --list");
    println!();
    println!("Options:");
    println!("  --iface <name>  Interface name (required for configuration)");
    println!("  --ip <addr>     Set IPv4 address");
    println!("  --mask <addr>    Set netmask");
    println!("  --gw <addr>      Set default gateway");
    println!("  --list           Show network configuration (interfaces, IP, gateway, MAC)");
    println!("  --help, -h       Show this help message");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = env::args_vec();
    let mut iface: Option<&str> = None;
    let mut ip: Option<Ipv4Address> = None;
    let mut mask: Option<Ipv4Address> = None;
    let mut gw: Option<Ipv4Address> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--list" => {
                match list_interfaces() {
                    Ok((status, interfaces)) => {
                        if status.gateway_set == 1 {
                            println!(
                                "Gateway: {}.{}.{}.{}",
                                status.gateway[0],
                                status.gateway[1],
                                status.gateway[2],
                                status.gateway[3]
                            );
                        } else {
                            println!("Gateway: (none)");
                        }

                        println!(
                            "Netmask: {}.{}.{}.{}",
                            status.netmask[0],
                            status.netmask[1],
                            status.netmask[2],
                            status.netmask[3]
                        );
                        println!();

                        if interfaces.is_empty() {
                            println!("(no interfaces)");
                            return 0;
                        }

                        for info in &interfaces {
                            let name_bytes = &info.name;
                            let null_pos = name_bytes
                                .iter()
                                .position(|&b| b == 0)
                                .unwrap_or(name_bytes.len());
                            let name = core::str::from_utf8(&name_bytes[..null_pos])
                                .unwrap_or("(invalid)");

                            println!("Interface: {}", name);
                            if info.ip_set == 1 {
                                println!(
                                    "  IP: {}.{}.{}.{}",
                                    info.ip_address[0],
                                    info.ip_address[1],
                                    info.ip_address[2],
                                    info.ip_address[3]
                                );
                            } else {
                                println!("  IP: (none)");
                            }
                            println!(
                                "  MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                                info.mac_address[0],
                                info.mac_address[1],
                                info.mac_address[2],
                                info.mac_address[3],
                                info.mac_address[4],
                                info.mac_address[5]
                            );
                        }
                    }
                    Err(_) => {
                        println!("netcfg: failed to list interfaces");
                        return 1;
                    }
                }
                return 0;
            }
            "--iface" => {
                i += 1;
                iface = args.get(i).map(|s| s.as_str());
            }
            "--ip" => {
                i += 1;
                ip = args.get(i).and_then(|s| parse_ipv4(s));
            }
            "--mask" => {
                i += 1;
                mask = args.get(i).and_then(|s| parse_ipv4(s));
            }
            "--gw" => {
                i += 1;
                gw = args.get(i).and_then(|s| parse_ipv4(s));
            }
            "--help" | "-h" => {
                print_usage();
                return 0;
            }
            unknown => {
                println!("netcfg: unknown option {}", unknown);
                print_usage();
                return 1;
            }
        }
        i += 1;
    }

    let iface = match iface {
        Some(name) => name,
        None => {
            println!("netcfg: --iface is required");
            print_usage();
            return 1;
        }
    };

    let mut failed = false;
    if let Some(ip) = ip
        && set_interface_ipv4(iface, ip).is_err()
    {
        println!("netcfg: failed to set ip (iface not found?)");
        failed = true;
    }
    if let Some(mask) = mask
        && set_netmask(mask).is_err()
    {
        println!("netcfg: failed to set mask");
        failed = true;
    }
    if let Some(gw) = gw
        && set_default_gateway(gw).is_err()
    {
        println!("netcfg: failed to set gateway");
        failed = true;
    }
    if failed { 1 } else { 0 }
}
