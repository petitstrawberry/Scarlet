//! Network page actions run outside the UI thread; wirelessd owns the radio.
use super::*;
use scarlet_os::wireless_service as api;
use zeroize::Zeroize;
#[path = "netcfgd/config.rs"]
mod policy;
const CONFIG_DIR: &str = "/etc/netcfgd.d";
#[derive(Clone, Debug)]
pub(super) struct Interface {
    name: String,
    pub label: String,
    pub detail: String,
}
static REQUESTING: AtomicBool = AtomicBool::new(false);
pub(super) fn start_monitor(app: &SettingsApp) {
    let network_app = app.clone();
    let app = app.clone();
    thread::spawn(move || {
        loop {
            match api::call(api::STATUS, &[], &[]) {
                Ok(text) => {
                    let mut display = String::new();
                    for line in text.lines() {
                        if let Some(value) = line.strip_prefix("STATE ") {
                            display.push_str(value);
                        }
                        if let Some(value) = line.strip_prefix("SSID ") {
                            if let Some(ssid) = api::unhex(value) {
                                if !ssid.is_empty() {
                                    display.push_str(&format!(
                                        "\nNetwork: {}",
                                        String::from_utf8_lossy(&ssid)
                                    ));
                                }
                            }
                        }
                        if let Some(value) = line.strip_prefix("LINK ") {
                            display.push_str(&format!("\n{}", value));
                        }
                    }
                    app.wifi_status.set(display);
                    app.wifi_networks.set(api::networks(&text));
                    app.wifi_can_scan
                        .set(text.lines().any(|l| l == "CAN_SCAN 1"));
                }
                Err(e) => app.wifi_status.set(String::from(e)),
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
    let app = network_app;
    thread::spawn(move || {
        loop {
            if let (Ok(links), Ok(configs)) = (
                scarlet_os::network::list_links(),
                scarlet_os::network::list_interface_configs(),
            ) {
                let mut routes: Vec<_> = configs
                    .iter()
                    .filter(|c| {
                        c.ip_set != 0
                            && c.gateway_set != 0
                            && links
                                .iter()
                                .any(|l| l.interface_name() == c.interface_name() && l.is_up())
                    })
                    .collect();
                routes.sort_by_key(|c| c.metric);
                let best_metric = routes.first().map(|c| c.metric);
                let tied = routes
                    .iter()
                    .filter(|c| Some(c.metric) == best_metric)
                    .count()
                    > 1;
                let preferred = if tied {
                    None
                } else {
                    routes.first().and_then(|c| c.interface_name())
                };
                let mut routing = String::new();
                if tied {
                    routing.push_str("Equal-priority default routes. Set different metrics to choose a preferred interface.\n");
                }
                for (index, route) in routes.iter().enumerate() {
                    if index != 0 {
                        routing.push('\n');
                    }
                    routing.push_str(&format!(
                        "{}: {} via {} · metric {}",
                        if tied && Some(route.metric) == best_metric {
                            "Default candidate"
                        } else if index == 0 {
                            "Default route"
                        } else {
                            "Fallback route"
                        },
                        route.interface_name().unwrap_or("?"),
                        ipv4(route.gateway),
                        route.metric
                    ));
                }
                if routing.is_empty() {
                    routing.push_str("No active default route");
                }
                app.network_routes.set(routing);
                let interfaces: Vec<_> = links
                    .iter()
                    .filter_map(|link| {
                        let name = link.interface_name()?;
                        let config = configs.iter().find(|c| c.interface_name() == Some(name));
                        let kind = if link.kind == 2 { "Wi-Fi" } else { "Ethernet" };
                        let mut detail = format!(
                            "{} · {}",
                            kind,
                            if link.is_up() { "Link up" } else { "Link down" }
                        );
                        if let Some(c) = config {
                            if c.ip_set != 0 {
                                detail.push_str(&format!(
                                    "\nIPv4: {}/{}",
                                    ipv4(c.ip_address),
                                    u32::from_be_bytes(c.netmask).count_ones()
                                ));
                            } else {
                                detail.push_str("\nIPv4: Not configured");
                            }
                            if c.gateway_set != 0 {
                                detail.push_str(&format!(
                                    "\nGateway: {} · metric {}",
                                    ipv4(c.gateway),
                                    c.metric
                                ));
                            }
                        }
                        Some(Interface {
                            name: String::from(name),
                            label: format!(
                                "{} ({}{})",
                                name,
                                kind,
                                if preferred == Some(name) {
                                    " · default route"
                                } else {
                                    ""
                                }
                            ),
                            detail,
                        })
                    })
                    .collect();
                let selected = app.network_name.get();
                let index = interfaces.iter().position(|i| i.name == selected);
                app.network_interfaces.set(interfaces);
                if let Some(index) = index {
                    app.network_index.set(index);
                } else if !app.network_interfaces.get().is_empty() {
                    app.select_network(0);
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}
fn ipv4(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}
impl SettingsApp {
    pub(super) fn select_network(&self, index: usize) {
        let Some(interface) = self.network_interfaces.get().get(index).cloned() else {
            return;
        };
        self.network_index.set(index);
        self.network_name.set(interface.name.clone());
        let config = policy::load_config_dir(CONFIG_DIR)
            .ok()
            .and_then(|c| policy::select_interface_config(&c, &interface.name))
            .unwrap_or_else(policy::InterfaceConfig::default_dhcp);
        self.network_method.set(match config.method {
            policy::InterfaceMethod::Dhcp => 0,
            policy::InterfaceMethod::Static => 1,
            policy::InterfaceMethod::Disabled => 2,
        });
        self.network_address.set(
            config
                .address
                .map(|ip| {
                    format!(
                        "{}/{}",
                        ipv4(ip.0),
                        config
                            .netmask
                            .map_or(24, |m| u32::from_be_bytes(m.0).count_ones())
                    )
                })
                .unwrap_or_default(),
        );
        self.network_gateway
            .set(config.gateway.map(|ip| ipv4(ip.0)).unwrap_or_default());
        self.network_dns.set(
            config
                .dns_servers
                .iter()
                .map(|ip| ipv4(ip.0))
                .collect::<Vec<_>>()
                .join(", "),
        );
        self.network_metric.set(format!("{}", config.metric));
        self.network_status.set(String::new());
    }
    pub(super) fn apply_network(&self) {
        let name = self.network_name.get();
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            self.network_status.set(String::from("Select an interface"));
            return;
        }
        let method = self.network_method.get();
        let address = self.network_address.get();
        let gateway = self.network_gateway.get();
        let dns = self.network_dns.get();
        let metric = self.network_metric.get();
        let app = self.clone();
        thread::spawn(move || {
            let result = (|| {
                let metric = metric
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| String::from("Enter a numeric route metric"))?;
                let method_name = match method {
                    0 => "dhcp",
                    1 => "static",
                    _ => "disabled",
                };
                let mut text = format!(
                    "[[interface]]\nname = \"{}\"\nmethod = \"{}\"\nmetric = {}\nrequired = false\n",
                    name, method_name, metric
                );
                let preferred = policy::load_config_dir(CONFIG_DIR)
                    .ok()
                    .and_then(|c| policy::select_interface_config(&c, &name))
                    .is_some_and(|c| c.default_route);
                text.push_str(&format!("default = {}\n", preferred));
                if method == 1 {
                    let address = address.trim();
                    let gateway = gateway.trim();
                    if !address.contains('/')
                        || !address
                            .bytes()
                            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'/'))
                    {
                        return Err(String::from(
                            "Enter IPv4 address/prefix, for example 192.168.1.20/24",
                        ));
                    }
                    text.push_str(&format!("address = \"{}\"\n", address));
                    if !gateway.is_empty() {
                        if !valid_ipv4(gateway) {
                            return Err(String::from("Enter a valid IPv4 gateway"));
                        }
                        text.push_str(&format!("gateway = \"{}\"\n", gateway));
                    }
                }
                let dns: Vec<_> = dns
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .collect();
                if dns.iter().any(|s| !valid_ipv4(s)) {
                    return Err(String::from("Enter IPv4 DNS servers separated by commas"));
                }
                text.push_str(&format!(
                    "dns_servers = [{}]\n",
                    dns.iter()
                        .map(|s| format!("\"{}\"", s))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                let mut parsed = policy::NetworkConfig::default();
                policy::parse_config_fragment(&text, "Network settings", &mut parsed)?;
                let path = format!("{}/zz-settings-{}.toml", CONFIG_DIR, name);
                atomic_write(&path, &format!("{}.tmp", path), text.as_bytes())
                    .map_err(String::from)?;
                Ok::<_, String>(())
            })();
            app.network_status.set(match result {
                Ok(()) => String::from("Saved. Active settings refresh automatically."),
                Err(e) => e,
            });
        });
    }
    pub(super) fn wifi_action(&self, command: u8) {
        if REQUESTING.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        let mut ssid = self.wifi_ssid.get().into_bytes();
        if let Some(network) = self.wifi_networks.get().get(self.wifi_index.get()) {
            if String::from_utf8_lossy(&network.ssid).as_ref()
                == core::str::from_utf8(&ssid).unwrap_or("")
            {
                ssid = network.ssid.clone();
            }
        }
        let mut password = self.wifi_password.get();
        if command == api::CONNECT {
            self.wifi_password.set(String::new());
        }
        let app = self.clone();
        thread::spawn(move || {
            let result = if command == api::CONNECT {
                api::call(command, &ssid, password.as_bytes())
            } else {
                api::call(command, &[], &[])
            };
            password.zeroize();
            app.wifi_status.set(String::from(match result {
                Ok(_) => "Request accepted",
                Err(e) => e,
            }));
            REQUESTING.store(false, AtomicOrdering::Release);
        });
    }
}
fn valid_ipv4(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u8>().is_ok()
        })
}
