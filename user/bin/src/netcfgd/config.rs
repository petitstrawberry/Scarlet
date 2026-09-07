//! Configuration parsing for `netcfgd`.

use std::{
    format,
    fs::{File, list_directory},
    network::Ipv4Address,
    string::{String, ToString},
    vec::Vec,
};

/// Address acquisition method for an interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfaceMethod {
    /// Acquire IPv4 configuration with DHCP.
    Dhcp,
    /// Apply an explicitly configured IPv4 address.
    Static,
    /// Leave the interface unconfigured.
    Disabled,
}

/// Parsed configuration for one interface selector.
#[derive(Clone, Debug)]
pub struct InterfaceConfig {
    /// Interface name or `*` wildcard.
    pub name: String,
    /// Address acquisition method.
    pub method: InterfaceMethod,
    /// Static IPv4 address.
    pub address: Option<Ipv4Address>,
    /// Static IPv4 netmask.
    pub netmask: Option<Ipv4Address>,
    /// Static default gateway.
    pub gateway: Option<Ipv4Address>,
    /// DNS servers that override DHCP-provided servers.
    pub dns_servers: Vec<Ipv4Address>,
    /// Default route metric.
    pub metric: u32,
    /// Prefer this interface for unbound sockets.
    pub default_route: bool,
    /// Treat failure to configure this interface as fatal.
    pub required: bool,
    /// Optional per-interface DHCP response timeout.
    pub dhcp_timeout_ms: Option<u64>,
    /// Optional per-interface DHCP attempt count.
    pub dhcp_attempts: Option<u32>,
}

impl InterfaceConfig {
    /// Create the built-in DHCP wildcard configuration.
    ///
    /// # Returns
    ///
    /// A configuration that attempts DHCP on every otherwise-unmatched
    /// interface without making boot depend on network availability.
    pub fn default_dhcp() -> Self {
        Self {
            name: "*".to_string(),
            method: InterfaceMethod::Dhcp,
            address: None,
            netmask: None,
            gateway: None,
            dns_servers: Vec::new(),
            metric: 100,
            default_route: false,
            required: false,
            dhcp_timeout_ms: None,
            dhcp_attempts: None,
        }
    }
}

/// Global `netcfgd` settings and interface selectors.
#[derive(Debug)]
pub struct NetworkConfig {
    /// Resolver configuration generated after interfaces are configured.
    pub resolv_conf: String,
    /// Default timeout for one DHCP response.
    pub dhcp_timeout_ms: u64,
    /// Default number of DHCP discovery attempts.
    pub dhcp_attempts: u32,
    /// Interface configuration selectors in file order.
    pub interfaces: Vec<InterfaceConfig>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            resolv_conf: "/etc/resolv.conf".to_string(),
            dhcp_timeout_ms: 1_500,
            dhcp_attempts: 3,
            interfaces: Vec::new(),
        }
    }
}

#[derive(Default)]
struct InterfaceBuilder {
    name: String,
    method: Option<InterfaceMethod>,
    address: Option<Ipv4Address>,
    address_prefix: Option<u8>,
    netmask: Option<Ipv4Address>,
    gateway: Option<Ipv4Address>,
    dns_servers: Vec<Ipv4Address>,
    metric: Option<u32>,
    default_route: Option<bool>,
    required: Option<bool>,
    dhcp_timeout_ms: Option<u64>,
    dhcp_attempts: Option<u32>,
}

impl InterfaceBuilder {
    fn finish(self, source: &str) -> Result<InterfaceConfig, String> {
        if self.name.is_empty() {
            return Err(format!("{source}: interface entry has no name"));
        }

        let method = self.method.unwrap_or(if self.address.is_some() {
            InterfaceMethod::Static
        } else {
            InterfaceMethod::Dhcp
        });
        let prefix_mask = self.address_prefix.map(prefix_to_netmask);
        if let (Some(from_prefix), Some(explicit)) = (prefix_mask, self.netmask)
            && from_prefix != explicit
        {
            return Err(format!(
                "{source}: address prefix and netmask disagree for {}",
                self.name
            ));
        }
        let netmask = self.netmask.or(prefix_mask);

        if method == InterfaceMethod::Static {
            if self.address.is_none() {
                return Err(format!(
                    "{source}: static interface {} requires address",
                    self.name
                ));
            }
            if self.address.is_some_and(is_invalid_unicast) {
                return Err(format!(
                    "{source}: static interface {} has an unusable address",
                    self.name
                ));
            }
            if netmask.is_none() {
                return Err(format!(
                    "{source}: static interface {} requires an address prefix or netmask",
                    self.name
                ));
            }
        }
        if method != InterfaceMethod::Static
            && (self.address.is_some() || self.netmask.is_some() || self.gateway.is_some())
        {
            return Err(format!(
                "{source}: {} settings cannot be combined with method {:?}",
                self.name, method
            ));
        }
        if self.gateway.is_some_and(is_invalid_unicast) {
            return Err(format!(
                "{source}: interface {} has an unusable gateway",
                self.name
            ));
        }
        if self.dns_servers.iter().copied().any(is_invalid_unicast) {
            return Err(format!(
                "{source}: interface {} has an unusable DNS server",
                self.name
            ));
        }

        Ok(InterfaceConfig {
            name: self.name,
            method,
            address: self.address,
            netmask,
            gateway: self.gateway,
            dns_servers: self.dns_servers,
            metric: self.metric.unwrap_or(100),
            default_route: self.default_route.unwrap_or(false),
            required: self.required.unwrap_or(false),
            dhcp_timeout_ms: self.dhcp_timeout_ms,
            dhcp_attempts: self.dhcp_attempts,
        })
    }
}

/// Load all TOML fragments in a directory in lexical order.
///
/// # Arguments
///
/// * `directory` - Configuration directory to read.
///
/// # Returns
///
/// The merged global and interface configuration, or an explanatory error.
pub fn load_config_dir(directory: &str) -> Result<NetworkConfig, String> {
    let entries = list_directory(directory)
        .map_err(|_| format!("failed to read configuration directory {directory}"))?;
    let mut filenames = Vec::new();
    for entry in entries {
        if entry.is_file() && entry.name.ends_with(".toml") {
            filenames.push(entry.name);
        }
    }
    filenames.sort();

    let mut config = NetworkConfig::default();
    for filename in filenames {
        let path = format!("{directory}/{filename}");
        let content = read_file(&path)?;
        parse_config_fragment(&content, &path, &mut config)?;
    }
    Ok(config)
}

/// Select the effective entry for an available interface.
///
/// Exact selectors take precedence over wildcard selectors. Within the same
/// selector class, later configuration fragments override earlier ones.
///
/// # Arguments
///
/// * `config` - Merged daemon configuration.
/// * `interface` - Available interface name.
///
/// # Returns
///
/// A cloned effective configuration, or `None` when no selector matches.
pub fn select_interface_config(config: &NetworkConfig, interface: &str) -> Option<InterfaceConfig> {
    config
        .interfaces
        .iter()
        .rev()
        .find(|entry| entry.name == interface)
        .or_else(|| {
            config
                .interfaces
                .iter()
                .rev()
                .find(|entry| entry.name == "*")
        })
        .cloned()
}

fn read_file(path: &str) -> Result<String, String> {
    let mut file = File::open(path).map_err(|_| format!("failed to open {path}"))?;
    let mut content = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => content.extend_from_slice(&buffer[..count]),
            Err(_) => return Err(format!("failed to read {path}")),
        }
    }
    String::from_utf8(content).map_err(|_| format!("{path}: configuration is not UTF-8"))
}

fn parse_config_fragment(
    content: &str,
    source: &str,
    config: &mut NetworkConfig,
) -> Result<(), String> {
    let mut current_interface: Option<InterfaceBuilder> = None;
    let mut in_network_section = false;

    for (line_index, raw_line) in content.lines().enumerate() {
        let line_number = line_index + 1;
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if line == "[[interface]]" {
            if let Some(builder) = current_interface.take() {
                config.interfaces.push(builder.finish(source)?);
            }
            current_interface = Some(InterfaceBuilder::default());
            in_network_section = false;
            continue;
        }
        if line.starts_with('[') {
            if let Some(builder) = current_interface.take() {
                config.interfaces.push(builder.finish(source)?);
            }
            if line != "[network]" {
                return Err(format!(
                    "{source}:{line_number}: unsupported section {line}"
                ));
            }
            in_network_section = true;
            continue;
        }

        let (key, raw_value) = line
            .split_once('=')
            .ok_or_else(|| format!("{source}:{line_number}: expected key = value"))?;
        let key = key.trim();
        let value = raw_value.trim();
        if let Some(interface) = current_interface.as_mut() {
            parse_interface_property(interface, key, value, source, line_number)?;
        } else if in_network_section {
            parse_network_property(config, key, value, source, line_number)?;
        } else {
            return Err(format!(
                "{source}:{line_number}: property outside a supported section"
            ));
        }
    }

    if let Some(builder) = current_interface {
        config.interfaces.push(builder.finish(source)?);
    }
    Ok(())
}

fn parse_network_property(
    config: &mut NetworkConfig,
    key: &str,
    value: &str,
    source: &str,
    line: usize,
) -> Result<(), String> {
    match key {
        "resolv_conf" => config.resolv_conf = parse_string(value, source, line)?,
        "dhcp_timeout_ms" => {
            config.dhcp_timeout_ms = parse_u64(value, source, line)?;
            if config.dhcp_timeout_ms == 0 {
                return Err(format!("{source}:{line}: dhcp_timeout_ms must be positive"));
            }
        }
        "dhcp_attempts" => {
            config.dhcp_attempts = parse_u32(value, source, line)?;
            if config.dhcp_attempts == 0 {
                return Err(format!("{source}:{line}: dhcp_attempts must be positive"));
            }
        }
        _ => return Err(format!("{source}:{line}: unknown network property {key}")),
    }
    Ok(())
}

fn parse_interface_property(
    interface: &mut InterfaceBuilder,
    key: &str,
    value: &str,
    source: &str,
    line: usize,
) -> Result<(), String> {
    match key {
        "name" => interface.name = parse_string(value, source, line)?,
        "method" | "mode" => {
            let method = parse_string(value, source, line)?;
            interface.method = Some(match method.as_str() {
                "dhcp" | "auto" => InterfaceMethod::Dhcp,
                "static" => InterfaceMethod::Static,
                "disabled" | "none" => InterfaceMethod::Disabled,
                _ => return Err(format!("{source}:{line}: unknown method {method}")),
            });
        }
        "address" => {
            let address = parse_string(value, source, line)?;
            let (parsed, prefix) = parse_address_with_prefix(&address)
                .ok_or_else(|| format!("{source}:{line}: invalid IPv4 address {address}"))?;
            interface.address = Some(parsed);
            interface.address_prefix = prefix;
        }
        "netmask" => {
            let mask = parse_string(value, source, line)?;
            let parsed = parse_ipv4(&mask)
                .filter(|value| is_valid_netmask(*value))
                .ok_or_else(|| format!("{source}:{line}: invalid IPv4 netmask {mask}"))?;
            interface.netmask = Some(parsed);
        }
        "gateway" => {
            let gateway = parse_string(value, source, line)?;
            interface.gateway = Some(
                parse_ipv4(&gateway)
                    .ok_or_else(|| format!("{source}:{line}: invalid gateway {gateway}"))?,
            );
        }
        "dns" | "dns_servers" => {
            let values = parse_string_array(value, source, line)?;
            let mut servers = Vec::new();
            for server in values {
                servers.push(
                    parse_ipv4(&server)
                        .ok_or_else(|| format!("{source}:{line}: invalid DNS server {server}"))?,
                );
            }
            interface.dns_servers = servers;
        }
        "metric" => interface.metric = Some(parse_u32(value, source, line)?),
        "default" | "default_route" => {
            interface.default_route = Some(parse_bool(value, source, line)?);
        }
        "required" => interface.required = Some(parse_bool(value, source, line)?),
        "dhcp_timeout_ms" => {
            let timeout = parse_u64(value, source, line)?;
            if timeout == 0 {
                return Err(format!("{source}:{line}: dhcp_timeout_ms must be positive"));
            }
            interface.dhcp_timeout_ms = Some(timeout);
        }
        "dhcp_attempts" => {
            let attempts = parse_u32(value, source, line)?;
            if attempts == 0 {
                return Err(format!("{source}:{line}: dhcp_attempts must be positive"));
            }
            interface.dhcp_attempts = Some(attempts);
        }
        _ => {
            return Err(format!("{source}:{line}: unknown interface property {key}"));
        }
    }
    Ok(())
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut quote = '\0';
    for (index, character) in line.char_indices() {
        if character == '"' || character == '\'' {
            if quoted && character == quote {
                quoted = false;
            } else if !quoted {
                quoted = true;
                quote = character;
            }
        } else if character == '#' && !quoted {
            return &line[..index];
        }
    }
    line
}

fn parse_string(value: &str, source: &str, line: usize) -> Result<String, String> {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        Ok(value[1..value.len() - 1].to_string())
    } else {
        Err(format!("{source}:{line}: expected a quoted string"))
    }
}

fn parse_string_array(value: &str, source: &str, line: usize) -> Result<Vec<String>, String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| format!("{source}:{line}: expected a string array"))?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut values = Vec::new();
    for item in inner.split(',') {
        values.push(parse_string(item.trim(), source, line)?);
    }
    Ok(values)
}

fn parse_bool(value: &str, source: &str, line: usize) -> Result<bool, String> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{source}:{line}: expected true or false")),
    }
}

fn parse_u32(value: &str, source: &str, line: usize) -> Result<u32, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{source}:{line}: expected an unsigned integer"))
}

fn parse_u64(value: &str, source: &str, line: usize) -> Result<u64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{source}:{line}: expected an unsigned integer"))
}

fn parse_address_with_prefix(value: &str) -> Option<(Ipv4Address, Option<u8>)> {
    if let Some((address, prefix)) = value.split_once('/') {
        let prefix = prefix.parse::<u8>().ok()?;
        if prefix > 32 {
            return None;
        }
        Some((parse_ipv4(address)?, Some(prefix)))
    } else {
        Some((parse_ipv4(value)?, None))
    }
}

/// Parse a dotted-decimal IPv4 address.
///
/// # Arguments
///
/// * `value` - Textual IPv4 address.
///
/// # Returns
///
/// The parsed address, or `None` if the text is malformed.
pub fn parse_ipv4(value: &str) -> Option<Ipv4Address> {
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

fn prefix_to_netmask(prefix: u8) -> Ipv4Address {
    let bits = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Ipv4Address(bits.to_be_bytes())
}

fn is_valid_netmask(mask: Ipv4Address) -> bool {
    let bits = u32::from_be_bytes(mask.0);
    bits.leading_ones() + bits.trailing_zeros() == u32::BITS
}

fn is_invalid_unicast(address: Ipv4Address) -> bool {
    address.0 == [0; 4] || address.0 == [255; 4]
}
