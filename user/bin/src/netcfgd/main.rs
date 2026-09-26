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
mod lifecycle;

use core::cmp::Ordering;
use core::time::Duration;

use config::{InterfaceConfig, InterfaceMethod, NetworkConfig};
use scarlet_os::time::monotonic_time_ns;
use std::{
    env, format,
    fs::OpenOptions,
    io::Write,
    network::{
        Ipv4Address, NetworkLinkInfoV1, NetworkUpdateIpv4V1, list_interface_configs, list_links,
        update_link_ipv4,
    },
    println,
    socket::Socket,
    string::{String, ToString},
    sync::{Arc, Mutex},
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
use scarlet_abi::network::{IPV4_CLEAR, IPV4_HAS_GATEWAY, IPV4_MAKE_DEFAULT};

#[derive(Clone, Debug)]
struct AvailableInterface {
    name: String,
    mac_address: [u8; 6],
    link: NetworkLinkInfoV1,
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
    link: NetworkLinkInfoV1,
    preference: RoutePreference,
    servers: Vec<Ipv4Address>,
    domain_name: Option<String>,
}

struct ManagedInterface {
    interface: AvailableInterface,
    config: InterfaceConfig,
    preference: RoutePreference,
    lease: Option<dhcp::DhcpLease>,
    configured: bool,
    acquired_at_ns: u64,
    next_action_ns: u64,
}

struct DhcpCompletion {
    result: Result<dhcp::DhcpLease, String>,
    rejected: bool,
    completed_at_ns: u64,
}
struct DhcpJob {
    link: NetworkLinkInfoV1,
    name: String,
    output: Arc<Mutex<Option<DhcpCompletion>>>,
    thread: thread::JoinHandle,
}

fn available_interfaces() -> Result<Vec<AvailableInterface>, &'static str> {
    list_links()
        .map_err(|_| "failed to list network links")?
        .into_iter()
        .map(|link| {
            Ok(AvailableInterface {
                name: link
                    .interface_name()
                    .ok_or("invalid link name")?
                    .to_string(),
                mac_address: link.mac_address,
                link,
            })
        })
        .collect()
}

fn apply_ipv4(
    link: &NetworkLinkInfoV1,
    address: Ipv4Address,
    netmask: Ipv4Address,
    gateway: Option<Ipv4Address>,
    metric: u32,
    make_default: bool,
) -> Result<(), &'static str> {
    update_link_ipv4(&NetworkUpdateIpv4V1 {
        id: link.id,
        generation: link.generation,
        address: address.0,
        netmask: netmask.0,
        gateway: gateway.map_or([0; 4], |ip| ip.0),
        metric,
        flags: u32::from(gateway.is_some()) * IPV4_HAS_GATEWAY
            | u32::from(make_default) * IPV4_MAKE_DEFAULT,
        reserved: 0,
    })
    .map_err(|_| "link changed or IPv4 configuration failed")
}

fn clear_link(link: &NetworkLinkInfoV1) -> Result<(), &'static str> {
    update_link_ipv4(&NetworkUpdateIpv4V1 {
        id: link.id,
        generation: link.generation,
        flags: IPV4_CLEAR,
        ..Default::default()
    })
    .map_err(|_| "link changed while clearing IPv4")
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
    apply_ipv4(
        &interface.link,
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
        link: interface.link,
        preference: RoutePreference {
            explicitly_default: config.default_route,
            metric: config.metric,
            order: 0,
        },
        servers: config.dns_servers.clone(),
        domain_name: None,
    })
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
    let temporary = format!("{path}.netcfgd.tmp");
    let mut file = options
        .open(temporary.as_str())
        .map_err(|_| "failed to open resolver configuration")?;
    if file.write_all(content.as_bytes()).is_err() || file.flush().is_err() {
        return Err("failed to write resolver configuration");
    }
    drop(file);
    if std::fs::rename(temporary.as_str(), path).is_err() {
        // Scarlet's initramfs overlay does not implement rename yet. Keep
        // diskless distributions functional; the complete content is prepared
        // before truncating the destination. This fallback is not atomic.
        let mut destination = options
            .open(path)
            .map_err(|_| "failed to open resolver destination")?;
        destination
            .write_all(content.as_bytes())
            .map_err(|_| "failed to publish resolver configuration")?;
        let _ = std::fs::remove_file(temporary.as_str());
    }
    println!("netcfgd: Wrote {} DNS server(s) to {}", servers.len(), path);
    Ok(())
}

fn renewal_source(
    interface: &AvailableInterface,
    config: &InterfaceConfig,
    preference: &RoutePreference,
    lease: &dhcp::DhcpLease,
) -> ResolverSource {
    ResolverSource {
        interface_name: interface.name.clone(),
        link: interface.link,
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
    apply_ipv4(
        &preferred.link,
        Ipv4Address(record.ip_address),
        Ipv4Address(record.netmask),
        (record.gateway_set != 0).then_some(Ipv4Address(record.gateway)),
        preferred.preference.metric,
        true,
    )
    .map_err(|_| "failed to select the preferred interface")
}

fn install_maintained_lease(
    managed: &mut ManagedInterface,
    resolver_sources: &mut Vec<ResolverSource>,
    lease: dhcp::DhcpLease,
    now: u64,
) -> bool {
    let make_default = is_preferred_active_source(
        &managed.preference,
        &managed.interface.name,
        resolver_sources,
    );
    if apply_ipv4(
        &managed.interface.link,
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
            &managed.interface,
            &managed.config,
            &managed.preference,
            &lease,
        ),
    );
    managed.lease = Some(lease);
    managed.configured = true;
    true
}

/// Workers only exchange DHCP packets. The coordinator alone installs addresses,
/// routes and DNS, after checking the link incarnation again in the kernel.
fn start_dhcp_job(
    managed: &ManagedInterface,
    global: &NetworkConfig,
) -> Result<DhcpJob, &'static str> {
    let output = Arc::new(Mutex::new(None));
    let result_slot = output.clone();
    let interface = managed.interface.clone();
    let timeout = managed
        .config
        .dhcp_timeout_ms
        .unwrap_or(global.dhcp_timeout_ms);
    let attempts = managed
        .config
        .dhcp_attempts
        .unwrap_or(global.dhcp_attempts)
        .max(1);
    let lease = managed.lease.clone();
    let elapsed = monotonic_time_ns().saturating_sub(managed.acquired_at_ns);
    let thread = thread::Builder::new().spawn(move || {
        let (result, rejected) = if let Some(lease) = lease {
            let rebind = elapsed >= seconds_to_nanoseconds(u64::from(lease.rebinding_time_secs));
            match dhcp::renew(
                &interface.name,
                interface.mac_address,
                &lease,
                timeout,
                rebind,
            ) {
                Ok(lease) => (Ok(lease), false),
                Err(error) => {
                    let rejected = error.is_rejected();
                    (Err(error.to_string()), rejected)
                }
            }
        } else {
            (
                dhcp::acquire(&interface.name, interface.mac_address, timeout, attempts),
                false,
            )
        };
        *result_slot.lock() = Some(DhcpCompletion {
            result,
            rejected,
            completed_at_ns: monotonic_time_ns(),
        });
    })?;
    Ok(DhcpJob {
        link: managed.interface.link,
        name: managed.interface.name.clone(),
        output,
        thread,
    })
}

fn maintain_interfaces(configuration: &NetworkConfig) -> ! {
    let mut managed: Vec<ManagedInterface> = Vec::new();
    let mut jobs: Vec<DhcpJob> = Vec::new();
    let mut sources: Vec<ResolverSource> = Vec::new();
    let mut ready = false;
    let mut resolver_dirty = true;
    let mut cleared_down: Vec<NetworkLinkInfoV1> = Vec::new();
    println!("netcfgd: Monitoring link lifecycle");
    loop {
        let now = monotonic_time_ns();
        let interfaces = match available_interfaces() {
            Ok(interfaces) => interfaces,
            Err(error) => {
                // A failed query is not an empty snapshot: preserve state and retry.
                println!("netcfgd: {}", error);
                thread::sleep(Duration::from_secs(LEASE_POLL_INTERVAL_SECS));
                continue;
            }
        };
        let links: Vec<_> = interfaces.iter().map(|interface| interface.link).collect();
        cleared_down.retain(|old| links.iter().any(|link| link.same_incarnation(old)));
        for interface in &interfaces {
            if !interface.link.is_up()
                && config::select_interface_config(configuration, &interface.name)
                    .is_some_and(|config| config.method != InterfaceMethod::Disabled)
                && !cleared_down
                    .iter()
                    .any(|link| link.same_incarnation(&interface.link))
                && clear_link(&interface.link).is_ok()
            {
                cleared_down.push(interface.link);
            }
        }
        // Remove old generations before accepting any worker completion.
        managed.retain(|item| {
            if lifecycle::current_ready_link(&item.interface.link, &links) {
                return true;
            }
            if let Some(current) = links.iter().find(|link| link.id == item.interface.link.id) {
                if clear_link(current).is_err() {
                    return true;
                } // retry cleanup next poll
            }
            sources.retain(|source| source.interface_name != item.interface.name);
            resolver_dirty = true;
            println!(
                "netcfgd: {} link lost/changed; cleared address, route and DNS",
                item.interface.name
            );
            false
        });
        for (order, interface) in interfaces.iter().enumerate() {
            if !interface.link.is_up()
                || managed
                    .iter()
                    .any(|item| item.interface.link.id == interface.link.id)
            {
                continue;
            }
            let Some(config) = config::select_interface_config(configuration, &interface.name)
            else {
                continue;
            };
            if config.method == InterfaceMethod::Disabled {
                continue;
            }
            // A fresh daemon also discards configuration left by a previous owner.
            if clear_link(&interface.link).is_err() {
                continue;
            }
            println!(
                "netcfgd: {} link ready id={} generation={}",
                interface.name, interface.link.id, interface.link.generation
            );
            managed.push(ManagedInterface {
                interface: interface.clone(),
                config: config.clone(),
                preference: RoutePreference {
                    explicitly_default: config.default_route,
                    metric: config.metric,
                    order,
                },
                lease: None,
                configured: false,
                acquired_at_ns: 0,
                next_action_ns: now,
            });
        }
        let mut index = 0;
        while index < jobs.len() {
            let finished = jobs[index].thread.try_join();
            if matches!(finished, Ok(false)) {
                index += 1;
                continue;
            }
            let job = jobs.remove(index);
            let completion = job.output.lock().take();
            if !lifecycle::current_ready_link(&job.link, &links) {
                continue;
            }
            let Some(item) = managed
                .iter_mut()
                .find(|item| item.interface.link.same_incarnation(&job.link))
            else {
                continue;
            };
            let Some(completion) = completion else {
                item.next_action_ns =
                    now.saturating_add(seconds_to_nanoseconds(LEASE_RETRY_INTERVAL_SECS));
                continue;
            };
            match completion.result {
                Ok(lease) => {
                    // Never install a lease that expired while its worker was descheduled.
                    if now.saturating_sub(completion.completed_at_ns)
                        < seconds_to_nanoseconds(u64::from(lease.lease_time_secs))
                        && install_maintained_lease(
                            item,
                            &mut sources,
                            lease,
                            completion.completed_at_ns,
                        )
                    {
                        println!(
                            "netcfgd: {} DHCP configuration installed",
                            item.interface.name
                        );
                        resolver_dirty = true;
                        continue;
                    }
                }
                Err(error) => println!("netcfgd: {} DHCP failed: {}", item.interface.name, error),
            }
            if completion.rejected && clear_link(&item.interface.link).is_ok() {
                item.lease = None;
                item.configured = false;
                sources.retain(|source| source.interface_name != item.interface.name);
                resolver_dirty = true;
            }
            item.next_action_ns =
                now.saturating_add(seconds_to_nanoseconds(LEASE_RETRY_INTERVAL_SECS));
        }
        for item in &mut managed {
            if !lifecycle::current_ready_link(&item.interface.link, &links) {
                continue;
            }
            if item.lease.as_ref().is_some_and(|lease| {
                now.saturating_sub(item.acquired_at_ns)
                    >= seconds_to_nanoseconds(u64::from(lease.lease_time_secs))
            }) {
                if clear_link(&item.interface.link).is_err() {
                    continue;
                }
                item.lease = None;
                item.configured = false;
                item.next_action_ns = now;
                sources.retain(|source| source.interface_name != item.interface.name);
                resolver_dirty = true;
                // Invalidate a renewal still in flight by keeping its output detached.
                for job in &mut jobs {
                    if job.link.same_incarnation(&item.interface.link) {
                        job.link.generation = 0;
                    }
                }
            }
            if now < item.next_action_ns {
                continue;
            }
            match item.config.method {
                InterfaceMethod::Static if !item.configured => {
                    let default = is_preferred_active_source(
                        &item.preference,
                        &item.interface.name,
                        &sources,
                    );
                    match configure_static_interface(&item.interface, &item.config, default) {
                        Ok(mut source) => {
                            source.preference = item.preference.clone();
                            replace_resolver_source(&mut sources, source);
                            item.configured = true;
                            resolver_dirty = true;
                        }
                        Err(error) => println!("netcfgd: {}: {}", item.interface.name, error),
                    }
                    item.next_action_ns =
                        now.saturating_add(seconds_to_nanoseconds(LEASE_RETRY_INTERVAL_SECS));
                }
                InterfaceMethod::Dhcp => {
                    // Includes obsolete generations until their bounded worker exits:
                    // never race two DHCP sockets on the same interface name.
                    if jobs.iter().any(|job| job.name == item.interface.name) {
                        continue;
                    }
                    match start_dhcp_job(item, configuration) {
                        Ok(job) => jobs.push(job),
                        Err(error) => println!("netcfgd: cannot start DHCP: {}", error),
                    }
                    item.next_action_ns =
                        now.saturating_add(seconds_to_nanoseconds(LEASE_RETRY_INTERVAL_SECS));
                }
                _ => (),
            }
        }
        if resolver_dirty {
            // Drop preference/DNS for every down interface before selecting a fallback.
            let preferred = prefer_best_configured_interface(&sources);
            let resolver = write_resolver_configuration(&configuration.resolv_conf, &sources);
            if let Err(error) = &preferred {
                println!("netcfgd: {}", error);
            }
            if let Err(error) = &resolver {
                println!("netcfgd: {}", error);
            }
            resolver_dirty = preferred.is_err() || resolver.is_err();
        }
        if !ready
            && !resolver_dirty
            && required_interfaces_ready(configuration, &interfaces, &managed)
        {
            notify_service_ready();
            ready = true;
        }
        thread::sleep(Duration::from_secs(LEASE_POLL_INTERVAL_SECS));
    }
}

fn required_interfaces_ready(
    configuration: &NetworkConfig,
    interfaces: &[AvailableInterface],
    managed: &[ManagedInterface],
) -> bool {
    // Explicit required names must exist, even before their first carrier-up.
    for entry in &configuration.interfaces {
        if entry.name != "*"
            && config::select_interface_config(configuration, &entry.name).is_some_and(|selected| {
                selected.required && selected.method != InterfaceMethod::Disabled
            })
            && !interfaces
                .iter()
                .any(|interface| interface.name == entry.name)
        {
            return false;
        }
    }
    if interfaces.is_empty()
        && configuration
            .interfaces
            .iter()
            .rev()
            .find(|entry| entry.name == "*")
            .is_some_and(|entry| entry.required && entry.method != InterfaceMethod::Disabled)
    {
        return false;
    }
    interfaces.iter().all(|interface| {
        let Some(entry) = config::select_interface_config(configuration, &interface.name) else {
            return true;
        };
        !entry.required
            || entry.method == InterfaceMethod::Disabled
            || managed.iter().any(|item| {
                item.configured
                    && item.interface.link.same_incarnation(&interface.link)
                    && interface.link.is_up()
            })
    })
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
                // Wait until stemd has latched readiness before continuing.
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

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("netcfgd: Network configuration starting");
    let arguments = env::args_vec();
    let directory = arguments
        .get(1)
        .map(|value| value.as_str())
        .unwrap_or(DEFAULT_CONFIG_DIRECTORY);
    match load_configuration(directory) {
        Ok(configuration) => maintain_interfaces(&configuration),
        Err(error) => {
            println!("netcfgd: Invalid configuration: {}", error);
            1
        }
    }
}
