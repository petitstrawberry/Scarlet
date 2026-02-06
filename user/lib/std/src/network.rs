//! User-space network configuration helpers for Scarlet Native.

use crate::handle::HandleError;
use crate::syscall::{Syscall, syscall1, syscall2};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    pub fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }
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

pub fn list_interfaces(buffer: &mut [u8]) -> Result<usize, HandleError> {
    let result = syscall2(
        Syscall::NetworkListInterfaces,
        buffer.as_mut_ptr() as usize,
        buffer.len(),
    );
    HandleError::from_syscall_result(result).map(|size| size as usize)
}
