//! Network Configuration Daemon (`netcfgd`).
//!
//! `netcfgd` reads TOML fragments from `/etc/netcfgd.d`, configures every
//! matching interface with DHCP or static IPv4 settings, and generates the
//! resolver configuration from the same source of truth.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

mod config;
mod dhcp;

use core::cmp::Ordering;
use core::time::Duration;

use config::{InterfaceConfig, InterfaceMethod, NetworkConfig};
use scarlet_os::time::monotonic_time_ns;
use std::{
    env, format,
    fs::OpenOptions,
    io::{Read, Write},
    network::{
        Ipv4Address, clear_interface_ipv4, configure_interface_ipv4, list_interface_configs,
    },
    println,
    socket::Socket,
    string::{String, ToString},
    thread,
    vec::Vec,
};

const DEFAULT_CONFIG_DIRECTORY: &str = "/etc/netcfgd.d";
const STEMD_SOCKET_PATH: &str = "/tmp/stemd.sock";
const STEMD_SERVICE_READY: u8 = 0x06;
const READY_NOTIFY_ATTEMPTS: usize = 20;
const READY_NOTIFY_DELAY_MS: u64 = 25;
const LEASE_RETRY_INTERVAL_SECS: u64 = 30;
const LEASE_POLL_INTERVAL_SECS: u64 = 1;

#[derive(Clone, Debug)]
struct AvailableInterface {
    name: String,
    mac_address: [u8; 6],
}

#[derive(Clone, Debug)]
struct RoutePreference {
    explicitly_default: bool,
    metric: u32,
    order: usize,
}

impl RoutePreference {
    fn compare(&self, other: &Self) -> Ordering {
        other
            .explicitly_default
            .cmp(&self.explicitly_default)
            .then(self.metric.cmp(&other.metric))
            .then(self.order.cmp(&other.order))
    }
}

#[derive(Clone, Debug)]
struct ResolverSource {
    interface_name: String,
    preference: RoutePreference,
    servers: Vec<Ipv4Address>,
    domain_name: Option<String>,
}

#[derive(Debug)]
struct ManagedDhcpLease {
    interface: AvailableInterface,
    config: InterfaceConfig,
    preference: RoutePreference,
    timeout_ms: u64,
    attempts: u32,
    lease: Option<dhcp::DhcpLease>,
    acquired_at_ns: u64,
    next_action_ns: u64,
}

fn available_interfaces() -> Result<Vec<AvailableInterface>, &'static str> {
    let records = list_interface_configs().map_err(|_| "failed to list network interfaces")?;
    let mut interfaces = Vec::new();
    for record in records {
        let name = record
            .interface_name()
            .ok_or("kernel returned an invalid interface name")?;
        interfaces.push(AvailableInterface {
            name: name.to_string(),
            mac_address: record.mac_address,
        });
    }
    Ok(interfaces)
}

fn load_configuration(directory: &str) -> Result<NetworkConfig, String> {
    match config::load_config_dir(directory) {
        Ok(mut configuration) => {
            if configuration.interfaces.is_empty() {
                println!("netcfgd: No interface entries found; using DHCP for all interfaces");
                configuration
                    .interfaces
                    .push(InterfaceConfig::default_dhcp());
            }
            Ok(configuration)
        }
        Err(error) if error.starts_with("failed to read configuration directory") => {
            println!("netcfgd: {}. Using built-in DHCP configuration", error);
            let mut configuration = NetworkConfig::default();
            configuration
                .interfaces
                .push(InterfaceConfig::default_dhcp());
            Ok(configuration)
        }
        Err(error) => Err(error),
    }
}

fn configure_static_interface(
    interface: &AvailableInterface,
    config: &InterfaceConfig,
    make_default: bool,
) -> Result<ResolverSource, &'static str> {
    let address = config.address.ok_or("static address is missing")?;
    let netmask = config.netmask.ok_or("static netmask is missing")?;
    configure_interface_ipv4(
        &interface.name,
        address,
        netmask,
        config.gateway,
        config.metric,
        make_default,
    )
    .map_err(|_| "kernel rejected static IPv4 configuration")?;

    println!(
        "netcfgd: {} configured as {} / {}",
        interface.name,
        format_ipv4(address),
        prefix_length(netmask)
    );
    if let Some(gateway) = config.gateway {
        println!(
            "netcfgd: {} default route via {} metric {}",
            interface.name,
            format_ipv4(gateway),
            config.metric
        );
    }

    Ok(ResolverSource {
        interface_name: interface.name.clone(),
        preference: RoutePreference {
            explicitly_default: config.default_route,
            metric: config.metric,
            order: 0,
        },
        servers: config.dns_servers.clone(),
        domain_name: None,
    })
}

fn configure_dhcp_interface(
    interface: &AvailableInterface,
    config: &InterfaceConfig,
    global: &NetworkConfig,
    make_default: bool,
) -> Result<(ResolverSource, dhcp::DhcpLease), String> {
    let timeout_ms = config.dhcp_timeout_ms.unwrap_or(global.dhcp_timeout_ms);
    let attempts = config.dhcp_attempts.unwrap_or(global.dhcp_attempts).max(1);
    println!(
        "netcfgd: {} requesting DHCP lease ({} attempt(s), {} ms timeout)",
        interface.name, attempts, timeout_ms
    );
    let lease = dhcp::acquire(&interface.name, interface.mac_address, timeout_ms, attempts)?;

    configure_interface_ipv4(
        &interface.name,
        lease.address,
        lease.netmask,
        lease.gateway,
        config.metric,
        make_default,
    )
    .map_err(|_| "kernel rejected DHCP IPv4 configuration".to_string())?;

    println!(
        "netcfgd: {} leased {} / {} from {} (lease {}s, T1 {}s, T2 {}s)",
        interface.name,
        format_ipv4(lease.address),
        prefix_length(lease.netmask),
        format_ipv4(lease.server_identifier),
        lease.lease_time_secs,
        lease.renewal_time_secs,
        lease.rebinding_time_secs
    );
    if let Some(gateway) = lease.gateway {
        println!(
            "netcfgd: {} default route via {} metric {}",
            interface.name,
            format_ipv4(gateway),
            config.metric
        );
    }

    let servers = if config.dns_servers.is_empty() {
        lease.dns_servers.clone()
    } else {
        config.dns_servers.clone()
    };
    let source = ResolverSource {
        interface_name: interface.name.clone(),
        preference: RoutePreference {
            explicitly_default: config.default_route,
            metric: config.metric,
            order: 0,
        },
        servers,
        domain_name: lease.domain_name.clone(),
    };
    Ok((source, lease))
}

fn write_resolver_configuration(
    path: &str,
    sources: &[ResolverSource],
) -> Result<(), &'static str> {
    let mut sources = sources.to_vec();
    sources.sort_by(|left, right| left.preference.compare(&right.preference));

    let mut servers = Vec::new();
    let mut domain_name = None;
    for source in sources {
        if domain_name.is_none() {
            domain_name = source.domain_name;
        }
        for server in source.servers {
            if !servers.contains(&server) {
                servers.push(server);
            }
        }
    }

    let mut content = String::from("# Generated by netcfgd; do not edit.\n");
    if let Some(domain) = domain_name {
        content.push_str(&format!("search {domain}\n"));
    }
    for server in &servers {
        content.push_str(&format!("nameserver {}\n", format_ipv4(*server)));
    }

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options
        .open(path)
        .map_err(|_| "failed to open resolver configuration")?;
    if file.write_all(content.as_bytes()).is_err() || file.flush().is_err() {
        return Err("failed to write resolver configuration");
    }
    println!("netcfgd: Wrote {} DNS server(s) to {}", servers.len(), path);
    Ok(())
}

fn renewal_source(
    interface_name: &str,
    config: &InterfaceConfig,
    preference: &RoutePreference,
    lease: &dhcp::DhcpLease,
) -> ResolverSource {
    ResolverSource {
        interface_name: interface_name.to_string(),
        preference: preference.clone(),
        servers: if config.dns_servers.is_empty() {
            lease.dns_servers.clone()
        } else {
            config.dns_servers.clone()
        },
        domain_name: lease.domain_name.clone(),
    }
}

fn replace_resolver_source(sources: &mut Vec<ResolverSource>, replacement: ResolverSource) {
    if let Some(existing) = sources
        .iter_mut()
        .find(|source| source.interface_name == replacement.interface_name)
    {
        *existing = replacement;
    } else {
        sources.push(replacement);
    }
}

fn seconds_to_nanoseconds(seconds: u64) -> u64 {
    seconds.saturating_mul(1_000_000_000)
}

fn schedule_renewal(acquired_at_ns: u64, lease: &dhcp::DhcpLease) -> u64 {
    acquired_at_ns.saturating_add(seconds_to_nanoseconds(
        u64::from(lease.renewal_time_secs).max(1),
    ))
}

fn managed_dhcp_lease(
    interface: &AvailableInterface,
    config: &InterfaceConfig,
    global: &NetworkConfig,
    preference: RoutePreference,
    lease: Option<dhcp::DhcpLease>,
) -> ManagedDhcpLease {
    let now = monotonic_time_ns();
    let next_action_ns = lease.as_ref().map_or_else(
        || now.saturating_add(seconds_to_nanoseconds(LEASE_RETRY_INTERVAL_SECS)),
        |lease| schedule_renewal(now, lease),
    );
    ManagedDhcpLease {
        interface: interface.clone(),
        config: config.clone(),
        preference,
        timeout_ms: config.dhcp_timeout_ms.unwrap_or(global.dhcp_timeout_ms),
        attempts: config.dhcp_attempts.unwrap_or(global.dhcp_attempts).max(1),
        lease,
        acquired_at_ns: now,
        next_action_ns,
    }
}

fn is_preferred_active_source(
    candidate: &RoutePreference,
    interface_name: &str,
    sources: &[ResolverSource],
) -> bool {
    let current = sources
        .iter()
        .filter(|source| source.interface_name != interface_name)
        .map(|source| &source.preference)
        .min_by(|left, right| left.compare(right));
    should_make_default(candidate, current)
}

fn prefer_best_configured_interface(sources: &[ResolverSource]) -> Result<(), &'static str> {
    let Some(preferred) = sources
        .iter()
        .min_by(|left, right| left.preference.compare(&right.preference))
    else {
        return Ok(());
    };
    let records = list_interface_configs().map_err(|_| "failed to list interfaces")?;
    let record = records
        .iter()
        .find(|record| record.interface_name() == Some(preferred.interface_name.as_str()))
        .ok_or("preferred interface disappeared")?;
    if record.ip_set == 0 {
        return Err("preferred interface has no IPv4 address");
    }
    configure_interface_ipv4(
        &preferred.interface_name,
        Ipv4Address(record.ip_address),
        Ipv4Address(record.netmask),
        (record.gateway_set != 0).then_some(Ipv4Address(record.gateway)),
        preferred.preference.metric,
        true,
    )
    .map_err(|_| "failed to select the preferred interface")
}

fn install_maintained_lease(
    managed: &mut ManagedDhcpLease,
    resolver_sources: &mut Vec<ResolverSource>,
    lease: dhcp::DhcpLease,
    now: u64,
) -> bool {
    let make_default = is_preferred_active_source(
        &managed.preference,
        &managed.interface.name,
        resolver_sources,
    );
    if configure_interface_ipv4(
        &managed.interface.name,
        lease.address,
        lease.netmask,
        lease.gateway,
        managed.config.metric,
        make_default,
    )
    .is_err()
    {
        return false;
    }

    managed.acquired_at_ns = now;
    managed.next_action_ns = schedule_renewal(now, &lease);
    replace_resolver_source(
        resolver_sources,
        renewal_source(
            &managed.interface.name,
            &managed.config,
            &managed.preference,
            &lease,
        ),
    );
    managed.lease = Some(lease);
    true
}

fn maintain_dhcp_leases(
    resolv_conf: &str,
    leases: &mut [ManagedDhcpLease],
    resolver_sources: &mut Vec<ResolverSource>,
) -> ! {
    println!("netcfgd: Monitoring {} DHCP lease(s)", leases.len());
    loop {
        let now = monotonic_time_ns();
        let mut resolver_changed = false;

        for managed in leases.iter_mut() {
            if now < managed.next_action_ns {
                continue;
            }

            let Some(current_lease) = managed.lease.clone() else {
                match dhcp::acquire(
                    &managed.interface.name,
                    managed.interface.mac_address,
                    managed.timeout_ms,
                    managed.attempts,
                ) {
                    Ok(lease) => {
                        let address = lease.address;
                        if install_maintained_lease(managed, resolver_sources, lease, now) {
                            println!(
                                "netcfgd: {} reacquired DHCP address {}",
                                managed.interface.name,
                                format_ipv4(address)
                            );
                            resolver_changed = true;
                            continue;
                        }
                        println!(
                            "netcfgd: {} reacquired a lease but kernel configuration failed",
                            managed.interface.name
                        );
                    }
                    Err(error) => println!(
                        "netcfgd: {} DHCP reacquisition failed: {}",
                        managed.interface.name, error
                    ),
                }
                managed.next_action_ns =
                    now.saturating_add(seconds_to_nanoseconds(LEASE_RETRY_INTERVAL_SECS));
                continue;
            };

            let elapsed_ns = now.saturating_sub(managed.acquired_at_ns);
            let expires_ns = seconds_to_nanoseconds(u64::from(current_lease.lease_time_secs));
            if elapsed_ns >= expires_ns {
                println!(
                    "netcfgd: {} DHCP lease expired; removing its IPv4 configuration",
                    managed.interface.name
                );
                let _ = clear_interface_ipv4(&managed.interface.name);
                managed.lease = None;
                managed.next_action_ns = now;
                resolver_sources.retain(|source| source.interface_name != managed.interface.name);
                if let Err(error) = prefer_best_configured_interface(resolver_sources) {
                    println!("netcfgd: Failed to select fallback interface: {}", error);
                }
                resolver_changed = true;
                continue;
            }

            let rebind_ns = seconds_to_nanoseconds(u64::from(current_lease.rebinding_time_secs));
            let rebind = elapsed_ns >= rebind_ns;
            match dhcp::renew(
                &managed.interface.name,
                managed.interface.mac_address,
                &current_lease,
                managed.timeout_ms,
                rebind,
            ) {
                Ok(lease) => {
                    if install_maintained_lease(managed, resolver_sources, lease, now) {
                        println!(
                            "netcfgd: {} DHCP lease {}",
                            managed.interface.name,
                            if rebind { "rebound" } else { "renewed" }
                        );
                        resolver_changed = true;
                        continue;
                    }
                    println!(
                        "netcfgd: {} lease renewal could not be installed",
                        managed.interface.name
                    );
                }
                Err(error) if error.is_rejected() => {
                    println!(
                        "netcfgd: {} DHCP server rejected the lease; reacquiring",
                        managed.interface.name
                    );
                    let _ = clear_interface_ipv4(&managed.interface.name);
                    managed.lease = None;
                    managed.next_action_ns = now;
                    resolver_sources
                        .retain(|source| source.interface_name != managed.interface.name);
                    if let Err(error) = prefer_best_configured_interface(resolver_sources) {
                        println!("netcfgd: Failed to select fallback interface: {}", error);
                    }
                    resolver_changed = true;
                    continue;
                }
                Err(error) => println!(
                    "netcfgd: {} DHCP {} failed: {}",
                    managed.interface.name,
                    if rebind { "rebind" } else { "renewal" },
                    error
                ),
            }

            let expiry_deadline = managed.acquired_at_ns.saturating_add(expires_ns);
            let phase_deadline = if rebind {
                expiry_deadline
            } else {
                managed.acquired_at_ns.saturating_add(rebind_ns)
            };
            managed.next_action_ns = now
                .saturating_add(seconds_to_nanoseconds(LEASE_RETRY_INTERVAL_SECS))
                .min(phase_deadline)
                .min(expiry_deadline);
        }

        if resolver_changed
            && let Err(error) = write_resolver_configuration(resolv_conf, resolver_sources)
        {
            println!(
                "netcfgd: Failed to refresh resolver configuration: {}",
                error
            );
        }
        thread::sleep(Duration::from_secs(LEASE_POLL_INTERVAL_SECS));
    }
}

fn notify_service_ready() {
    for _ in 0..READY_NOTIFY_ATTEMPTS {
        if let Ok(socket) = Socket::new()
            && socket.connect(STEMD_SOCKET_PATH).is_ok()
            && let Ok(stream) = socket.as_stream()
        {
            let service_name = b"netcfgd";
            let mut payload = Vec::new();
            payload.push(STEMD_SERVICE_READY);
            payload.extend_from_slice(&(service_name.len() as u32).to_le_bytes());
            payload.extend_from_slice(service_name);
            if stream.write_all(&payload).is_ok() {
                // netcfgd is a one-shot service when it has no managed DHCP
                // leases. Do not exit until stemd has latched the notification;
                // otherwise PID 1 can reap us before its IPC worker records it.
                let mut response = [0u8; 32];
                if let Ok(length) = stream.read(&mut response)
                    && response[..length].starts_with(b"OK:")
                {
                    return;
                }
            }
        }
        thread::sleep(Duration::from_millis(READY_NOTIFY_DELAY_MS));
    }
    println!("netcfgd: Warning: failed to notify stemd readiness");
}

fn format_ipv4(address: Ipv4Address) -> String {
    format!(
        "{}.{}.{}.{}",
        address.0[0], address.0[1], address.0[2], address.0[3]
    )
}

fn prefix_length(netmask: Ipv4Address) -> u32 {
    u32::from_be_bytes(netmask.0).count_ones()
}

fn should_make_default(candidate: &RoutePreference, current: Option<&RoutePreference>) -> bool {
    current.is_none_or(|current| candidate.compare(current) == Ordering::Less)
}

fn warn_unmatched_entries(config: &NetworkConfig, interfaces: &[AvailableInterface]) -> usize {
    let mut required_missing = 0;
    for (index, entry) in config.interfaces.iter().enumerate() {
        if config.interfaces[index + 1..]
            .iter()
            .any(|later| later.name == entry.name)
        {
            continue;
        }
        let matched = if entry.name == "*" {
            !interfaces.is_empty()
        } else {
            interfaces.iter().any(|item| item.name == entry.name)
        };
        if !matched {
            println!(
                "netcfgd: Warning: configured interface {} is not present",
                entry.name
            );
            required_missing += usize::from(entry.required);
        }
    }
    required_missing
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("netcfgd: Network configuration starting");
    let arguments = env::args_vec();
    let config_directory = arguments
        .get(1)
        .map(|value| value.as_str())
        .unwrap_or(DEFAULT_CONFIG_DIRECTORY);

    let configuration = match load_configuration(config_directory) {
        Ok(configuration) => configuration,
        Err(error) => {
            println!("netcfgd: Invalid configuration: {}", error);
            return 1;
        }
    };
    let interfaces = match available_interfaces() {
        Ok(interfaces) => interfaces,
        Err(error) => {
            println!("netcfgd: {}", error);
            return 1;
        }
    };
    let required_missing = warn_unmatched_entries(&configuration, &interfaces);

    let mut configured_count = 0usize;
    let mut failed_count = required_missing;
    let mut required_failure = required_missing != 0;
    let mut default_preference: Option<RoutePreference> = None;
    let mut resolver_sources = Vec::new();
    let mut managed_leases = Vec::new();

    for (order, interface) in interfaces.iter().enumerate() {
        let Some(interface_config) =
            config::select_interface_config(&configuration, &interface.name)
        else {
            println!("netcfgd: {} has no matching configuration", interface.name);
            continue;
        };
        if interface_config.method == InterfaceMethod::Disabled {
            println!("netcfgd: {} is disabled by configuration", interface.name);
            continue;
        }

        let preference = RoutePreference {
            explicitly_default: interface_config.default_route,
            metric: interface_config.metric,
            order,
        };
        let make_default = should_make_default(&preference, default_preference.as_ref());
        let result = match interface_config.method {
            InterfaceMethod::Dhcp => {
                configure_dhcp_interface(interface, &interface_config, &configuration, make_default)
                    .map(|(source, lease)| (source, Some(lease)))
            }
            InterfaceMethod::Static => {
                configure_static_interface(interface, &interface_config, make_default)
                    .map_err(ToString::to_string)
                    .map(|source| (source, None))
            }
            InterfaceMethod::Disabled => unreachable!(),
        };

        match result {
            Ok((mut resolver_source, lease)) => {
                resolver_source.preference = preference.clone();
                resolver_sources.push(resolver_source);
                configured_count += 1;
                if make_default {
                    default_preference = Some(preference.clone());
                }
                if let Some(lease) = lease {
                    managed_leases.push(managed_dhcp_lease(
                        interface,
                        &interface_config,
                        &configuration,
                        preference,
                        Some(lease),
                    ));
                }
            }
            Err(error) => {
                println!("netcfgd: Failed to configure {}: {}", interface.name, error);
                failed_count += 1;
                required_failure |= interface_config.required;
                if interface_config.method == InterfaceMethod::Dhcp {
                    managed_leases.push(managed_dhcp_lease(
                        interface,
                        &interface_config,
                        &configuration,
                        preference,
                        None,
                    ));
                }
            }
        }
    }

    if let Err(error) = write_resolver_configuration(&configuration.resolv_conf, &resolver_sources)
    {
        println!("netcfgd: {}", error);
        return 1;
    }
    println!(
        "netcfgd: Configuration complete: {} configured, {} failed",
        configured_count, failed_count
    );

    if required_failure {
        println!("netcfgd: A required interface failed; service is not ready");
        return 1;
    }
    notify_service_ready();
    if managed_leases.is_empty() {
        0
    } else {
        maintain_dhcp_leases(
            &configuration.resolv_conf,
            &mut managed_leases,
            &mut resolver_sources,
        )
    }
}
