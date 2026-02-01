#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::network::{
    Ipv4Address, list_interfaces, set_default_gateway, set_dns_server, set_interface_ipv4,
    set_netmask,
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
    println!(
        "Usage: netcfg --iface <name> [--ip <addr>] [--mask <addr>] [--gw <addr>] [--dns <addr>]"
    );
    println!("       netcfg --list");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = env::args_vec();
    let mut iface: Option<&str> = None;
    let mut ip: Option<Ipv4Address> = None;
    let mut mask: Option<Ipv4Address> = None;
    let mut gw: Option<Ipv4Address> = None;
    let mut dns: Option<Ipv4Address> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--list" => {
                let mut buffer = [0u8; 256];
                match list_interfaces(&mut buffer) {
                    Ok(len) if len > 0 => {
                        if let Ok(text) = core::str::from_utf8(&buffer[..len]) {
                            println!("{}", text);
                        }
                    }
                    Ok(_) => {
                        println!("(no interfaces)");
                    }
                    Err(_) => {
                        println!("netcfg: failed to list interfaces");
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
            "--dns" => {
                i += 1;
                dns = args.get(i).and_then(|s| parse_ipv4(s));
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
    if let Some(ip) = ip {
        if let Err(_) = set_interface_ipv4(iface, ip) {
            println!("netcfg: failed to set ip (iface not found?)");
            failed = true;
        }
    }
    if let Some(mask) = mask {
        if let Err(_) = set_netmask(mask) {
            println!("netcfg: failed to set mask");
            failed = true;
        }
    }
    if let Some(gw) = gw {
        if let Err(_) = set_default_gateway(gw) {
            println!("netcfg: failed to set gateway");
            failed = true;
        }
    }
    if let Some(dns) = dns {
        if let Err(_) = set_dns_server(dns) {
            println!("netcfg: failed to set dns");
            failed = true;
        }
    }

    if failed { 1 } else { 0 }
}
