//! Minimal DHCPv4 client used by `netcfgd`.

use core::time::Duration;

use scarlet_os::time::monotonic_time_ns;
use std::{
    format,
    network::Ipv4Address,
    socket::{
        DatagramOps, Inet4SocketAddress, Socket, SocketAddress, SocketDomain, SocketError,
        SocketProtocol, SocketType,
    },
    string::{String, ToString},
    thread, vec,
    vec::Vec,
};

const DHCP_CLIENT_PORT: u16 = 68;
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
const BOOTP_FIXED_LEN: usize = 236;
const DHCP_OPTIONS_OFFSET: usize = BOOTP_FIXED_LEN + DHCP_MAGIC_COOKIE.len();
const MIN_DHCP_PACKET_LEN: usize = 300;
const MAX_DHCP_PACKET_LEN: usize = 1_500;
const POLL_INTERVAL_MS: u64 = 10;

const OPTION_PAD: u8 = 0;
const OPTION_SUBNET_MASK: u8 = 1;
const OPTION_ROUTER: u8 = 3;
const OPTION_DNS_SERVER: u8 = 6;
const OPTION_DOMAIN_NAME: u8 = 15;
const OPTION_REQUESTED_IP: u8 = 50;
const OPTION_LEASE_TIME: u8 = 51;
const OPTION_MESSAGE_TYPE: u8 = 53;
const OPTION_SERVER_IDENTIFIER: u8 = 54;
const OPTION_PARAMETER_REQUEST_LIST: u8 = 55;
const OPTION_MAX_MESSAGE_SIZE: u8 = 57;
const OPTION_RENEWAL_TIME: u8 = 58;
const OPTION_REBINDING_TIME: u8 = 59;
const OPTION_CLIENT_IDENTIFIER: u8 = 61;
const OPTION_END: u8 = 255;
const DHCP_REJECTED_MESSAGE: &str = "DHCP server rejected the requested lease";

/// DHCP-acquired IPv4 and resolver configuration.
#[derive(Clone, Debug)]
pub struct DhcpLease {
    /// Leased client IPv4 address.
    pub address: Ipv4Address,
    /// DHCP-provided subnet mask.
    pub netmask: Ipv4Address,
    /// First DHCP-provided router, if any.
    pub gateway: Option<Ipv4Address>,
    /// DHCP-provided DNS servers in server preference order.
    pub dns_servers: Vec<Ipv4Address>,
    /// DHCP-provided local domain name.
    pub domain_name: Option<String>,
    /// DHCP server identifier used for the lease.
    pub server_identifier: Ipv4Address,
    /// Lease lifetime in seconds.
    pub lease_time_secs: u32,
    /// Suggested renewal time in seconds.
    pub renewal_time_secs: u32,
    /// Suggested rebinding time in seconds.
    pub rebinding_time_secs: u32,
}

/// Failure returned while refreshing an existing DHCP lease.
#[derive(Debug)]
pub struct RenewalError {
    message: String,
    rejected: bool,
}

impl RenewalError {
    fn other(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rejected: false,
        }
    }

    /// Report whether the DHCP server explicitly rejected the current lease.
    ///
    /// # Returns
    ///
    /// `true` for DHCPNAK and `false` for timeouts or local socket failures.
    pub fn is_rejected(&self) -> bool {
        self.rejected
    }
}

impl core::fmt::Display for RenewalError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl MessageType {
    fn from_byte(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Discover),
            2 => Some(Self::Offer),
            3 => Some(Self::Request),
            4 => Some(Self::Decline),
            5 => Some(Self::Ack),
            6 => Some(Self::Nak),
            7 => Some(Self::Release),
            8 => Some(Self::Inform),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct DhcpReply {
    message_type: MessageType,
    offered_address: Ipv4Address,
    server_address: Ipv4Address,
    subnet_mask: Option<Ipv4Address>,
    gateway: Option<Ipv4Address>,
    dns_servers: Vec<Ipv4Address>,
    domain_name: Option<String>,
    lease_time_secs: Option<u32>,
    renewal_time_secs: Option<u32>,
    rebinding_time_secs: Option<u32>,
}

/// Acquire a DHCPv4 lease for one interface.
///
/// The socket is explicitly bound to the requested interface so acquisition
/// works before the interface has an IPv4 address and does not depend on the
/// process-global default interface.
///
/// # Arguments
///
/// * `interface` - Registered interface name.
/// * `mac_address` - Interface MAC address used as the DHCP client identity.
/// * `timeout_ms` - Time to wait for each offer or acknowledgement.
/// * `attempts` - Maximum discovery attempts.
///
/// # Returns
///
/// A validated DHCP lease, or a diagnostic error after all attempts fail.
pub fn acquire(
    interface: &str,
    mac_address: [u8; 6],
    timeout_ms: u64,
    attempts: u32,
) -> Result<DhcpLease, String> {
    let socket = Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Udp,
    )
    .map_err(|_| "failed to create DHCP UDP socket".to_string())?;
    socket
        .bind_interface(interface)
        .map_err(|_| format!("failed to bind DHCP socket to {interface}"))?;
    socket
        .bind_inet(Inet4SocketAddress::new([0, 0, 0, 0], DHCP_CLIENT_PORT))
        .map_err(|_| "failed to bind DHCP client port 68".to_string())?;
    socket
        .set_nonblocking(true)
        .map_err(|_| "failed to make DHCP socket non-blocking".to_string())?;

    let destination = SocketAddress::Inet(Inet4SocketAddress::new(
        [255, 255, 255, 255],
        DHCP_SERVER_PORT,
    ));
    let attempts = attempts.max(1);
    for attempt in 0..attempts {
        let transaction_id = transaction_id(mac_address, attempt);
        let discover = build_discover(transaction_id, mac_address);
        socket
            .sendto(&discover, &destination)
            .map_err(|_| "failed to send DHCPDISCOVER".to_string())?;

        let Some(offer) = wait_for_reply(
            &socket,
            transaction_id,
            mac_address,
            MessageType::Offer,
            timeout_ms,
        )?
        else {
            continue;
        };
        if is_invalid_unicast(offer.offered_address) || is_invalid_unicast(offer.server_address) {
            continue;
        }

        let request = build_request(
            transaction_id,
            mac_address,
            offer.offered_address,
            offer.server_address,
        );
        socket
            .sendto(&request, &destination)
            .map_err(|_| "failed to send DHCPREQUEST".to_string())?;

        let Some(ack) = wait_for_reply(
            &socket,
            transaction_id,
            mac_address,
            MessageType::Ack,
            timeout_ms,
        )?
        else {
            continue;
        };
        return Ok(merge_lease(offer, ack));
    }

    Err(format!(
        "no DHCP lease received after {attempts} attempt(s)"
    ))
}

/// Renew or rebind an existing DHCPv4 lease.
///
/// # Arguments
///
/// * `interface` - Interface that owns the lease.
/// * `mac_address` - Interface MAC address.
/// * `lease` - Current lease whose values are used when an ACK omits options.
/// * `timeout_ms` - Time to wait for an acknowledgement.
/// * `rebind` - Send a broadcast request instead of a unicast renewal.
///
/// # Returns
///
/// The refreshed lease on DHCPACK, or an error on timeout, DHCPNAK, or socket
/// failure.
pub fn renew(
    interface: &str,
    mac_address: [u8; 6],
    lease: &DhcpLease,
    timeout_ms: u64,
    rebind: bool,
) -> Result<DhcpLease, RenewalError> {
    let socket = Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Udp,
    )
    .map_err(|_| RenewalError::other("failed to create DHCP renewal socket"))?;
    socket.bind_interface(interface).map_err(|_| {
        RenewalError::other(format!("failed to bind DHCP renewal socket to {interface}"))
    })?;
    socket
        .bind_inet(Inet4SocketAddress::new(lease.address.0, DHCP_CLIENT_PORT))
        .map_err(|_| RenewalError::other("failed to bind DHCP renewal client port"))?;
    socket
        .set_nonblocking(true)
        .map_err(|_| RenewalError::other("failed to make DHCP renewal socket non-blocking"))?;

    let transaction_id = transaction_id(mac_address, lease.lease_time_secs);
    let request = build_renew_request(transaction_id, mac_address, lease.address);
    let destination_address = if rebind {
        [255, 255, 255, 255]
    } else {
        lease.server_identifier.0
    };
    socket
        .sendto(
            &request,
            &SocketAddress::Inet(Inet4SocketAddress::new(
                destination_address,
                DHCP_SERVER_PORT,
            )),
        )
        .map_err(|_| RenewalError::other("failed to send DHCP lease renewal"))?;

    let reply = wait_for_reply(
        &socket,
        transaction_id,
        mac_address,
        MessageType::Ack,
        timeout_ms,
    )
    .map_err(|message| RenewalError {
        rejected: message == DHCP_REJECTED_MESSAGE,
        message,
    })?;
    let Some(ack) = reply else {
        return Err(RenewalError::other("DHCP lease renewal timed out"));
    };
    Ok(merge_renewed_lease(lease, ack))
}

fn transaction_id(mac_address: [u8; 6], attempt: u32) -> u32 {
    let now = monotonic_time_ns();
    let mac_mix = u32::from_be_bytes([
        mac_address[2],
        mac_address[3],
        mac_address[4],
        mac_address[5],
    ]);
    (now as u32)
        .rotate_left(13)
        .wrapping_add((now >> 32) as u32)
        ^ mac_mix
        ^ attempt.wrapping_mul(0x9e37_79b9)
}

fn build_discover(transaction_id: u32, mac_address: [u8; 6]) -> Vec<u8> {
    let mut packet = boot_request(transaction_id, mac_address);
    push_option(
        &mut packet,
        OPTION_MESSAGE_TYPE,
        &[MessageType::Discover as u8],
    );
    push_client_identifier(&mut packet, mac_address);
    push_common_request_options(&mut packet);
    finish_options(&mut packet);
    packet
}

fn build_request(
    transaction_id: u32,
    mac_address: [u8; 6],
    requested_address: Ipv4Address,
    server_identifier: Ipv4Address,
) -> Vec<u8> {
    let mut packet = boot_request(transaction_id, mac_address);
    push_option(
        &mut packet,
        OPTION_MESSAGE_TYPE,
        &[MessageType::Request as u8],
    );
    push_option(&mut packet, OPTION_REQUESTED_IP, &requested_address.0);
    push_option(&mut packet, OPTION_SERVER_IDENTIFIER, &server_identifier.0);
    push_client_identifier(&mut packet, mac_address);
    push_common_request_options(&mut packet);
    finish_options(&mut packet);
    packet
}

fn build_renew_request(
    transaction_id: u32,
    mac_address: [u8; 6],
    current_address: Ipv4Address,
) -> Vec<u8> {
    let mut packet = boot_request(transaction_id, mac_address);
    packet[10..12].copy_from_slice(&0u16.to_be_bytes());
    packet[12..16].copy_from_slice(&current_address.0);
    push_option(
        &mut packet,
        OPTION_MESSAGE_TYPE,
        &[MessageType::Request as u8],
    );
    push_client_identifier(&mut packet, mac_address);
    push_common_request_options(&mut packet);
    finish_options(&mut packet);
    packet
}

fn boot_request(transaction_id: u32, mac_address: [u8; 6]) -> Vec<u8> {
    let mut packet = vec![0u8; DHCP_OPTIONS_OFFSET];
    packet[0] = 1;
    packet[1] = 1;
    packet[2] = mac_address.len() as u8;
    packet[4..8].copy_from_slice(&transaction_id.to_be_bytes());
    packet[10..12].copy_from_slice(&0x8000u16.to_be_bytes());
    packet[28..34].copy_from_slice(&mac_address);
    packet[BOOTP_FIXED_LEN..DHCP_OPTIONS_OFFSET].copy_from_slice(&DHCP_MAGIC_COOKIE);
    packet
}

fn push_client_identifier(packet: &mut Vec<u8>, mac_address: [u8; 6]) {
    let mut client_identifier = [0u8; 7];
    client_identifier[0] = 1;
    client_identifier[1..].copy_from_slice(&mac_address);
    push_option(packet, OPTION_CLIENT_IDENTIFIER, &client_identifier);
}

fn push_common_request_options(packet: &mut Vec<u8>) {
    push_option(
        packet,
        OPTION_PARAMETER_REQUEST_LIST,
        &[
            OPTION_SUBNET_MASK,
            OPTION_ROUTER,
            OPTION_DNS_SERVER,
            OPTION_DOMAIN_NAME,
            OPTION_LEASE_TIME,
            OPTION_SERVER_IDENTIFIER,
            OPTION_RENEWAL_TIME,
            OPTION_REBINDING_TIME,
        ],
    );
    push_option(packet, OPTION_MAX_MESSAGE_SIZE, &576u16.to_be_bytes());
}

fn push_option(packet: &mut Vec<u8>, code: u8, value: &[u8]) {
    packet.push(code);
    packet.push(value.len() as u8);
    packet.extend_from_slice(value);
}

fn finish_options(packet: &mut Vec<u8>) {
    packet.push(OPTION_END);
    if packet.len() < MIN_DHCP_PACKET_LEN {
        packet.resize(MIN_DHCP_PACKET_LEN, 0);
    }
}

fn wait_for_reply(
    socket: &Socket,
    transaction_id: u32,
    mac_address: [u8; 6],
    expected: MessageType,
    timeout_ms: u64,
) -> Result<Option<DhcpReply>, String> {
    let deadline = monotonic_time_ns().saturating_add(timeout_ms.saturating_mul(1_000_000));
    let mut buffer = [0u8; MAX_DHCP_PACKET_LEN];
    loop {
        match socket.recvfrom(&mut buffer) {
            Ok((length, _)) => {
                if let Some(reply) = parse_reply(&buffer[..length], transaction_id, mac_address) {
                    if reply.message_type == MessageType::Nak {
                        return Err(DHCP_REJECTED_MESSAGE.to_string());
                    }
                    if reply.message_type == expected {
                        return Ok(Some(reply));
                    }
                }
            }
            Err(SocketError::WouldBlock) => {}
            Err(_) => return Err("failed to receive DHCP response".to_string()),
        }

        if monotonic_time_ns() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
    }
}

fn parse_reply(packet: &[u8], transaction_id: u32, mac_address: [u8; 6]) -> Option<DhcpReply> {
    if packet.len() < DHCP_OPTIONS_OFFSET
        || packet[0] != 2
        || packet[1] != 1
        || packet[2] < mac_address.len() as u8
        || packet[4..8] != transaction_id.to_be_bytes()
        || packet[28..34] != mac_address
        || packet[BOOTP_FIXED_LEN..DHCP_OPTIONS_OFFSET] != DHCP_MAGIC_COOKIE
    {
        return None;
    }

    let offered_address = ipv4_from_slice(&packet[16..20])?;
    let boot_server = ipv4_from_slice(&packet[20..24])?;
    let mut message_type = None;
    let mut subnet_mask = None;
    let mut gateway = None;
    let mut dns_servers = Vec::new();
    let mut domain_name = None;
    let mut server_identifier = None;
    let mut lease_time_secs = None;
    let mut renewal_time_secs = None;
    let mut rebinding_time_secs = None;
    let mut offset = DHCP_OPTIONS_OFFSET;

    while offset < packet.len() {
        let code = packet[offset];
        offset += 1;
        if code == OPTION_PAD {
            continue;
        }
        if code == OPTION_END {
            break;
        }
        let length = *packet.get(offset)? as usize;
        offset += 1;
        let end = offset.checked_add(length)?;
        let value = packet.get(offset..end)?;
        offset = end;

        match code {
            OPTION_MESSAGE_TYPE if value.len() == 1 => {
                message_type = MessageType::from_byte(value[0]);
            }
            OPTION_SUBNET_MASK if value.len() == 4 => {
                subnet_mask = ipv4_from_slice(value).filter(|mask| is_valid_netmask(*mask));
            }
            OPTION_ROUTER if value.len() >= 4 => {
                gateway =
                    ipv4_from_slice(&value[..4]).filter(|address| !is_invalid_unicast(*address));
            }
            OPTION_DNS_SERVER => {
                for address in value.chunks_exact(4) {
                    if let Some(address) = ipv4_from_slice(address)
                        && !is_invalid_unicast(address)
                        && !dns_servers.contains(&address)
                    {
                        dns_servers.push(address);
                    }
                }
            }
            OPTION_DOMAIN_NAME => {
                if let Ok(value) = core::str::from_utf8(value)
                    && !value.is_empty()
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
                {
                    domain_name = Some(value.to_string());
                }
            }
            OPTION_SERVER_IDENTIFIER if value.len() == 4 => {
                server_identifier = ipv4_from_slice(value);
            }
            OPTION_LEASE_TIME if value.len() == 4 => {
                lease_time_secs = u32_from_slice(value);
            }
            OPTION_RENEWAL_TIME if value.len() == 4 => {
                renewal_time_secs = u32_from_slice(value);
            }
            OPTION_REBINDING_TIME if value.len() == 4 => {
                rebinding_time_secs = u32_from_slice(value);
            }
            _ => {}
        }
    }

    let server_address = server_identifier.unwrap_or(boot_server);
    Some(DhcpReply {
        message_type: message_type?,
        offered_address,
        server_address,
        subnet_mask,
        gateway,
        dns_servers,
        domain_name,
        lease_time_secs,
        renewal_time_secs,
        rebinding_time_secs,
    })
}

fn merge_lease(offer: DhcpReply, ack: DhcpReply) -> DhcpLease {
    let address = if is_invalid_unicast(ack.offered_address) {
        offer.offered_address
    } else {
        ack.offered_address
    };
    let lease_time_secs = ack
        .lease_time_secs
        .or(offer.lease_time_secs)
        .unwrap_or(3_600)
        .max(1);
    let renewal_time_secs = ack
        .renewal_time_secs
        .or(offer.renewal_time_secs)
        .unwrap_or(lease_time_secs / 2)
        .min(lease_time_secs);
    let rebinding_time_secs = ack
        .rebinding_time_secs
        .or(offer.rebinding_time_secs)
        .unwrap_or(lease_time_secs.saturating_mul(7) / 8)
        .max(renewal_time_secs)
        .min(lease_time_secs);

    DhcpLease {
        address,
        netmask: ack
            .subnet_mask
            .or(offer.subnet_mask)
            .unwrap_or_else(|| classful_netmask(address)),
        gateway: ack.gateway.or(offer.gateway),
        dns_servers: if ack.dns_servers.is_empty() {
            offer.dns_servers
        } else {
            ack.dns_servers
        },
        domain_name: ack.domain_name.or(offer.domain_name),
        server_identifier: if is_invalid_unicast(ack.server_address) {
            offer.server_address
        } else {
            ack.server_address
        },
        lease_time_secs,
        renewal_time_secs,
        rebinding_time_secs,
    }
}

fn merge_renewed_lease(current: &DhcpLease, ack: DhcpReply) -> DhcpLease {
    let address = if is_invalid_unicast(ack.offered_address) {
        current.address
    } else {
        ack.offered_address
    };
    let lease_time_secs = ack
        .lease_time_secs
        .unwrap_or(current.lease_time_secs)
        .max(1);
    let renewal_time_secs = ack
        .renewal_time_secs
        .unwrap_or(lease_time_secs / 2)
        .min(lease_time_secs);
    let rebinding_time_secs = ack
        .rebinding_time_secs
        .unwrap_or(lease_time_secs.saturating_mul(7) / 8)
        .max(renewal_time_secs)
        .min(lease_time_secs);

    DhcpLease {
        address,
        netmask: ack.subnet_mask.unwrap_or(current.netmask),
        gateway: ack.gateway.or(current.gateway),
        dns_servers: if ack.dns_servers.is_empty() {
            current.dns_servers.clone()
        } else {
            ack.dns_servers
        },
        domain_name: ack.domain_name.or_else(|| current.domain_name.clone()),
        server_identifier: if is_invalid_unicast(ack.server_address) {
            current.server_identifier
        } else {
            ack.server_address
        },
        lease_time_secs,
        renewal_time_secs,
        rebinding_time_secs,
    }
}

fn ipv4_from_slice(value: &[u8]) -> Option<Ipv4Address> {
    if value.len() != 4 {
        return None;
    }
    Some(Ipv4Address([value[0], value[1], value[2], value[3]]))
}

fn u32_from_slice(value: &[u8]) -> Option<u32> {
    if value.len() != 4 {
        return None;
    }
    Some(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
}

fn classful_netmask(address: Ipv4Address) -> Ipv4Address {
    match address.0[0] {
        0..=127 => Ipv4Address([255, 0, 0, 0]),
        128..=191 => Ipv4Address([255, 255, 0, 0]),
        _ => Ipv4Address([255, 255, 255, 0]),
    }
}

fn is_unspecified(address: Ipv4Address) -> bool {
    address.0 == [0, 0, 0, 0]
}

fn is_invalid_unicast(address: Ipv4Address) -> bool {
    is_unspecified(address) || address.0 == [255; 4]
}

fn is_valid_netmask(mask: Ipv4Address) -> bool {
    let bits = u32::from_be_bytes(mask.0);
    bits.leading_ones() + bits.trailing_zeros() == u32::BITS
}
