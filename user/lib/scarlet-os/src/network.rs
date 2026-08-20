//! Scarlet Native network configuration APIs.

extern crate alloc;

use alloc::vec::Vec;
use scarlet_sys::{Syscall, syscall1, syscall2};

use crate::handle::HandleError;

const NETWORK_CONFIGURE_HAS_GATEWAY: u32 = 1 << 0;
const NETWORK_CONFIGURE_MAKE_DEFAULT: u32 = 1 << 1;

/// An IPv4 address represented as four network-order octets.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    /// Construct an IPv4 address from four octets.
    ///
    /// # Arguments
    ///
    /// * `a` - First octet.
    /// * `b` - Second octet.
    /// * `c` - Third octet.
    /// * `d` - Fourth octet.
    ///
    /// # Returns
    ///
    /// The corresponding IPv4 address.
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NetworkConfigureIpv4Request {
    iface_ptr: usize,
    iface_len: usize,
    address: [u8; 4],
    netmask: [u8; 4],
    gateway: [u8; 4],
    flags: u32,
    metric: u32,
}

/// IPv4 configuration and link identity for one network interface.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetworkInterfaceConfig {
    /// NUL-padded interface name.
    pub name: [u8; 32],
    /// Primary IPv4 address.
    pub ip_address: [u8; 4],
    /// Netmask associated with the primary address.
    pub netmask: [u8; 4],
    /// Default gateway installed for this interface.
    pub gateway: [u8; 4],
    /// Link-layer MAC address.
    pub mac_address: [u8; 6],
    /// One when `ip_address` and `netmask` are configured.
    pub ip_set: u8,
    /// One when a default gateway is installed.
    pub gateway_set: u8,
    /// One when this is the preferred interface for unbound sockets.
    pub is_default: u8,
    reserved: [u8; 3],
    /// Default route metric, or zero when no default route is installed.
    pub metric: u32,
}

const _: [(); 40] = [(); core::mem::size_of::<NetworkConfigureIpv4Request>()];
const _: [(); 60] = [(); core::mem::size_of::<NetworkInterfaceConfig>()];

impl NetworkInterfaceConfig {
    /// Get the interface name stored in this record.
    ///
    /// # Returns
    ///
    /// The UTF-8 interface name, or `None` if the kernel returned invalid UTF-8.
    pub fn interface_name(&self) -> Option<&str> {
        let end = self
            .name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..end]).ok()
    }
}

/// Configure all IPv4 routing properties of one interface.
///
/// # Arguments
///
/// * `name` - Interface name to configure.
/// * `address` - Primary IPv4 address.
/// * `netmask` - Contiguous IPv4 network mask.
/// * `gateway` - Optional default gateway for this interface.
/// * `metric` - Default route metric; lower values are preferred.
/// * `make_default` - Whether unbound sockets should prefer this interface.
///
/// # Returns
///
/// `Ok(())` when the complete configuration is accepted by the kernel, or a
/// [`HandleError`] when validation or installation fails.
pub fn configure_interface_ipv4(
    name: &str,
    address: Ipv4Address,
    netmask: Ipv4Address,
    gateway: Option<Ipv4Address>,
    metric: u32,
    make_default: bool,
) -> Result<(), HandleError> {
    if name.is_empty() {
        return Err(HandleError::InvalidParameter);
    }

    let mut flags = 0;
    if gateway.is_some() {
        flags |= NETWORK_CONFIGURE_HAS_GATEWAY;
    }
    if make_default {
        flags |= NETWORK_CONFIGURE_MAKE_DEFAULT;
    }
    let request = NetworkConfigureIpv4Request {
        iface_ptr: name.as_ptr() as usize,
        iface_len: name.len(),
        address: address.0,
        netmask: netmask.0,
        gateway: gateway.map_or([0; 4], |value| value.0),
        flags,
        metric,
    };
    let result = syscall1(
        Syscall::NetworkConfigureIpv4,
        &request as *const NetworkConfigureIpv4Request as usize,
    );
    HandleError::from_syscall_result(result).map(|_| ())
}

/// List per-interface IPv4 configuration records.
///
/// # Returns
///
/// A record for every interface reported by the kernel, or a [`HandleError`]
/// if the query fails.
pub fn list_interface_configs() -> Result<Vec<NetworkInterfaceConfig>, HandleError> {
    const MAX_INTERFACES: usize = 32;
    let mut records = [NetworkInterfaceConfig {
        name: [0; 32],
        ip_address: [0; 4],
        netmask: [0; 4],
        gateway: [0; 4],
        mac_address: [0; 6],
        ip_set: 0,
        gateway_set: 0,
        is_default: 0,
        reserved: [0; 3],
        metric: 0,
    }; MAX_INTERFACES];
    let result = syscall2(
        Syscall::NetworkListInterfacesV2,
        records.as_mut_ptr() as usize,
        records.len(),
    );
    HandleError::from_syscall_result(result)?;

    let count = result.min(records.len());
    Ok(records[..count].to_vec())
}
