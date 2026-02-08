//! Network Configuration Daemon (netcfgd)
//!
//! This daemon reads TOML configuration files from /etc/netcfgd.d/
//! and applies network settings automatically. It is started by stemd during boot.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    env,
    fs::File,
    network::{
        Ipv4Address, list_interfaces, set_default_gateway, set_dns_server, set_interface_ipv4,
        set_netmask,
    },
    println,
    string::{String, ToString},
    vec::Vec,
};

/// Network interface configuration
#[derive(Debug)]
struct InterfaceConfig {
    name: String,
    address: Option<Ipv4Address>,
    netmask: Option<Ipv4Address>,
    gateway: Option<Ipv4Address>,
    dns: Option<Ipv4Address>,
}

/// Parse IPv4 address from string
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

/// Simple TOML parser for network configuration
struct ConfigParser {
    content: String,
}

impl ConfigParser {
    fn new(content: String) -> Self {
        Self { content }
    }

    /// Parse the configuration file and extract interface configurations
    fn parse(&self) -> Vec<InterfaceConfig> {
        let mut interfaces = Vec::new();
        let lines: Vec<&str> = self.content.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            if line.starts_with("[[interface]]") {
                let mut name = String::new();
                let mut address = None;
                let mut netmask = None;
                let mut gateway = None;
                let mut dns = None;

                i += 1;
                while i < lines.len() {
                    let prop_line = lines[i].trim();

                    if prop_line.is_empty() || prop_line.starts_with('[') {
                        break;
                    }

                    if prop_line.starts_with('#') {
                        i += 1;
                        continue;
                    }

                    if let Some(eq_pos) = prop_line.find('=') {
                        let key = prop_line[..eq_pos].trim();
                        let value = prop_line[eq_pos + 1..].trim();

                        match key {
                            "name" => {
                                name = Self::unquote(value);
                            }
                            "address" => {
                                if let Some(v) = Self::unquote(value).parse::<String>().ok() {
                                    address = parse_ipv4(&v);
                                }
                            }
                            "netmask" => {
                                if let Some(v) = Self::unquote(value).parse::<String>().ok() {
                                    netmask = parse_ipv4(&v);
                                }
                            }
                            "gateway" => {
                                if let Some(v) = Self::unquote(value).parse::<String>().ok() {
                                    gateway = parse_ipv4(&v);
                                }
                            }
                            "dns" => {
                                if let Some(v) = Self::unquote(value).parse::<String>().ok() {
                                    dns = parse_ipv4(&v);
                                }
                            }
                            _ => {}
                        }
                    }

                    i += 1;
                }

                if !name.is_empty() {
                    interfaces.push(InterfaceConfig {
                        name,
                        address,
                        netmask,
                        gateway,
                        dns,
                    });
                }

                continue;
            }

            i += 1;
        }

        interfaces
    }

    /// Remove surrounding quotes from a string
    fn unquote(s: &str) -> String {
        let s = s.trim();
        if ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
            && s.len() >= 2
        {
            return s[1..s.len() - 1].to_string();
        }
        s.to_string()
    }
}

/// Read configuration file
fn read_config(path: &str) -> Result<String, &'static str> {
    let mut file = File::open(path).map_err(|_| "Failed to open config file")?;

    let mut content = String::new();
    let mut buffer = [0u8; 4096];

    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => match core::str::from_utf8(&buffer[..n]) {
                Ok(s) => content.push_str(s),
                Err(_) => return Err("Config file contains invalid UTF-8"),
            },
            Err(_) => return Err("Failed to read config file"),
        }
    }

    Ok(content)
}

/// Read all configuration files from a directory
fn read_config_dir(dir_path: &str) -> Result<Vec<InterfaceConfig>, &'static str> {
    use std::fs::list_directory;

    let mut all_interfaces = Vec::new();

    match list_directory(dir_path) {
        Ok(entries) => {
            println!(
                "netcfgd: Reading configuration from directory: {}",
                dir_path
            );

            let mut toml_files = Vec::new();
            for entry in entries {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }

                if entry.is_file() && entry.name.ends_with(".toml") {
                    toml_files.push(entry.name);
                }
            }

            toml_files.sort();

            if toml_files.is_empty() {
                println!("netcfgd: No .toml files found in {}", dir_path);
                return Ok(Vec::new());
            }

            for filename in toml_files {
                use std::format;
                let file_path = format!("{}/{}", dir_path, filename);
                println!("netcfgd: Loading {}", file_path);

                match read_config(&file_path) {
                    Ok(content) => {
                        let parser = ConfigParser::new(content);
                        let interfaces = parser.parse();
                        all_interfaces.extend(interfaces);
                    }
                    Err(e) => {
                        println!("netcfgd: Warning: Failed to read {}: {}", file_path, e);
                    }
                }
            }

            Ok(all_interfaces)
        }
        Err(_) => Err("Failed to read configuration directory"),
    }
}

/// Apply network configuration for a single interface
fn apply_interface_config(config: &InterfaceConfig) -> Result<(), &'static str> {
    println!("netcfgd: Configuring interface: {}", config.name);

    let mut failed = false;

    if let Some(ip) = config.address {
        if set_interface_ipv4(&config.name, ip).is_err() {
            println!("netcfgd: Failed to set IP address for {}", config.name);
            failed = true;
        } else {
            println!(
                "netcfgd: Set IP address: {}.{}.{}.{}",
                ip.0[0], ip.0[1], ip.0[2], ip.0[3]
            );
        }
    }

    if let Some(mask) = config.netmask {
        if set_netmask(mask).is_err() {
            println!("netcfgd: Failed to set netmask");
            failed = true;
        } else {
            println!(
                "netcfgd: Set netmask: {}.{}.{}.{}",
                mask.0[0], mask.0[1], mask.0[2], mask.0[3]
            );
        }
    }

    if let Some(gw) = config.gateway {
        if set_default_gateway(gw).is_err() {
            println!("netcfgd: Failed to set default gateway");
            failed = true;
        } else {
            println!(
                "netcfgd: Set gateway: {}.{}.{}.{}",
                gw.0[0], gw.0[1], gw.0[2], gw.0[3]
            );
        }
    }

    if let Some(dns) = config.dns {
        if set_dns_server(dns).is_err() {
            println!("netcfgd: Failed to set DNS server");
            failed = true;
        } else {
            println!(
                "netcfgd: Set DNS: {}.{}.{}.{}",
                dns.0[0], dns.0[1], dns.0[2], dns.0[3]
            );
        }
    }

    if failed {
        Err("Failed to configure some network settings")
    } else {
        Ok(())
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("netcfgd: Network Configuration Daemon starting...");

    let args = env::args_vec();

    // Determine config directory path
    let config_dir = if args.len() > 1 {
        args[1].clone()
    } else {
        "/etc/netcfgd.d".to_string()
    };

    println!("netcfgd: Config directory: {}", config_dir);

    let interfaces = match read_config_dir(&config_dir) {
        Ok(interfaces) => interfaces,
        Err(e) => {
            println!("netcfgd: Error reading configuration: {}", e);
            return 0;
        }
    };

    if interfaces.is_empty() {
        println!("netcfgd: No network interfaces to configure");
        return 0;
    }

    let mut buffer = [0u8; 256];
    match list_interfaces(&mut buffer) {
        Ok(len) if len > 0 => {
            if let Ok(text) = core::str::from_utf8(&buffer[..len]) {
                println!("netcfgd: Available interfaces:\n{}", text);
            }
        }
        _ => {}
    }

    let mut failed_count = 0;
    for config in &interfaces {
        if let Err(e) = apply_interface_config(config) {
            println!("netcfgd: Failed to configure {}: {}", config.name, e);
            failed_count += 1;
        }
    }

    println!(
        "netcfgd: Configuration complete. Success: {}, Failed: {}",
        interfaces.len() - failed_count,
        failed_count
    );

    if failed_count > 0 { 1 } else { 0 }
}
