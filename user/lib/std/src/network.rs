//! User-space network configuration helpers for Scarlet Native.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::handle::HandleError;
use crate::syscall::{Syscall, syscall1, syscall3};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    pub fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetworkInterfaceInfo {
    pub name: [u8; 32],
    pub ip_address: [u8; 4],
    pub mac_address: [u8; 6],
    pub ip_set: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetworkStatus {
    pub gateway: [u8; 4],
    pub gateway_set: u8,
    pub dns_server: [u8; 4],
    pub dns_set: u8,
    pub netmask: [u8; 4],
    pub interface_count: u32,
    pub interfaces_ptr: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NetworkInterfaceAddress {
    iface_ptr: usize,
    iface_len: usize,
    addr: [u8; 4],
}

impl NetworkInterfaceAddress {
    pub fn new(name: &str, addr: Ipv4Address) -> Self {
        Self {
            iface_ptr: name.as_ptr() as usize,
            iface_len: name.len(),
            addr: addr.0,
        }
    }
}

pub fn set_interface_ipv4(name: &str, addr: Ipv4Address) -> Result<(), HandleError> {
    if name.is_empty() {
        return Err(HandleError::InvalidParameter);
    }
    let req = NetworkInterfaceAddress::new(name, addr);
    let result = syscall1(Syscall::NetworkSetIpv4, &req as *const _ as usize);
    HandleError::from_syscall_result(result).map(|_| ())
}

pub fn set_default_gateway(addr: Ipv4Address) -> Result<(), HandleError> {
    let result = syscall1(Syscall::NetworkSetGateway, addr.0.as_ptr() as usize);
    HandleError::from_syscall_result(result).map(|_| ())
}

pub fn set_dns_server(addr: Ipv4Address) -> Result<(), HandleError> {
    let result = syscall1(Syscall::NetworkSetDns, addr.0.as_ptr() as usize);
    HandleError::from_syscall_result(result).map(|_| ())
}

pub fn set_netmask(addr: Ipv4Address) -> Result<(), HandleError> {
    let result = syscall1(Syscall::NetworkSetNetmask, addr.0.as_ptr() as usize);
    HandleError::from_syscall_result(result).map(|_| ())
}

pub fn list_interfaces() -> Result<(NetworkStatus, Vec<NetworkInterfaceInfo>), HandleError> {
    let mut status: NetworkStatus = unsafe { core::mem::zeroed() };
    const MAX_INTERFACES: usize = 16;
    let mut interfaces = [unsafe { core::mem::zeroed::<NetworkInterfaceInfo>() }; MAX_INTERFACES];

    let result = syscall3(
        Syscall::NetworkListInterfaces,
        &mut status as *mut NetworkStatus as usize,
        interfaces.as_mut_ptr() as usize,
        MAX_INTERFACES,
    );

    HandleError::from_syscall_result(result)?;

    let count = status.interface_count as usize;
    let mut interface_list = Vec::new();

    for i in 0..count {
        let info = interfaces[i];
        let name_bytes = &info.name;
        let null_pos = name_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_bytes.len());
        let _name = match core::str::from_utf8(&name_bytes[..null_pos]) {
            Ok(s) => String::from(s),
            Err(_) => String::new(),
        };

        interface_list.push(NetworkInterfaceInfo {
            name: info.name,
            ip_address: info.ip_address,
            mac_address: info.mac_address,
            ip_set: info.ip_set,
        });
    }

    Ok((status, interface_list))
}
