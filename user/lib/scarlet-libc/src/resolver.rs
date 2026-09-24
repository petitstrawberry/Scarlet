//! IPv4 POSIX name resolution backed by Scarlet Rust std's resolver.

use std::ffi::{CStr, c_char, c_int};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};

const AF_UNSPEC: c_int = 0;
const AF_INET: c_int = 2;
const AF_INET6: c_int = 10;
const SOCK_STREAM: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const IPPROTO_TCP: c_int = 6;
const IPPROTO_UDP: c_int = 17;
const AI_PASSIVE: c_int = 0x01;
const AI_CANONNAME: c_int = 0x02;
const AI_NUMERICHOST: c_int = 0x04;
const AI_V4MAPPED: c_int = 0x08;
const AI_ALL: c_int = 0x10;
const AI_ADDRCONFIG: c_int = 0x20;
const AI_NUMERICSERV: c_int = 0x400;
const NI_NUMERICHOST: c_int = 0x01;
const NI_NUMERICSERV: c_int = 0x02;
const NI_NOFQDN: c_int = 0x04;
const NI_NAMEREQD: c_int = 0x08;
const NI_DGRAM: c_int = 0x10;
const EAI_BADFLAGS: c_int = -1;
const EAI_NONAME: c_int = -2;
const EAI_AGAIN: c_int = -3;
const EAI_FAIL: c_int = -4;
const EAI_FAMILY: c_int = -6;
const EAI_SOCKTYPE: c_int = -7;
const EAI_SERVICE: c_int = -8;
const EAI_MEMORY: c_int = -10;
const EAI_OVERFLOW: c_int = -12;

#[repr(C)]
pub struct AddrInfo {
    flags: c_int,
    family: c_int,
    socket_type: c_int,
    protocol: c_int,
    address_length: u32,
    address: *mut SockAddrIn,
    canonical_name: *mut c_char,
    next: *mut AddrInfo,
}

#[repr(C)]
pub struct SockAddrIn {
    family: u16,
    port: u16,
    address: [u8; 4],
    padding: [u8; 8],
}

unsafe fn c_string(pointer: *const c_char) -> Result<Option<String>, c_int> {
    if pointer.is_null() {
        return Ok(None);
    }
    // SAFETY: caller supplies a NUL-terminated string.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map(|text| Some(text.to_owned()))
        .map_err(|_| EAI_NONAME)
}

fn service_port(service: Option<&str>, flags: c_int) -> Result<u16, c_int> {
    let Some(service) = service else {
        return Ok(0);
    };
    if let Ok(port) = service.parse::<u16>() {
        return Ok(port);
    }
    if flags & AI_NUMERICSERV != 0 {
        return Err(EAI_NONAME);
    }
    match service {
        "http" => Ok(80),
        "https" => Ok(443),
        "ssh" => Ok(22),
        "domain" => Ok(53),
        _ => Err(EAI_SERVICE),
    }
}

unsafe fn allocate_c_string(value: &str) -> *mut c_char {
    let allocation = crate::allocation::malloc(value.len() + 1).cast::<c_char>();
    if !allocation.is_null() {
        // SAFETY: malloc provided value.len()+1 writable bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(value.as_ptr(), allocation.cast::<u8>(), value.len());
            *allocation.add(value.len()) = 0;
        }
    }
    allocation
}

/// # Safety
/// `node`, `service`, and non-null `hints` must be valid for reading; `result`
/// must be exclusively writable. The returned list belongs to freeaddrinfo.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn getaddrinfo(
    node: *const c_char,
    service: *const c_char,
    hints: *const AddrInfo,
    result: *mut *mut AddrInfo,
) -> c_int {
    if result.is_null() {
        return EAI_FAIL;
    }
    // SAFETY: caller supplies an exclusive result cell.
    unsafe { *result = std::ptr::null_mut() };
    // SAFETY: caller provides NUL-terminated input strings.
    let node = match unsafe { c_string(node) } {
        Ok(node) => node,
        Err(error) => return error,
    };
    let service = match unsafe { c_string(service) } {
        Ok(service) => service,
        Err(error) => return error,
    };
    if node.is_none() && service.is_none() {
        return EAI_NONAME;
    }
    // SAFETY: non-null hints are caller-owned and readable for this call.
    let hints = unsafe { hints.as_ref() };
    let flags = hints.map_or(0, |hints| hints.flags);
    if flags
        & !(AI_PASSIVE
            | AI_CANONNAME
            | AI_NUMERICHOST
            | AI_V4MAPPED
            | AI_ALL
            | AI_ADDRCONFIG
            | AI_NUMERICSERV)
        != 0
    {
        return EAI_BADFLAGS;
    }
    let family = hints.map_or(AF_UNSPEC, |hints| hints.family);
    if family == AF_INET6 {
        return EAI_FAMILY;
    }
    if family != AF_UNSPEC && family != AF_INET {
        return EAI_FAMILY;
    }
    let socket_type = hints.map_or(0, |hints| hints.socket_type);
    if !matches!(socket_type, 0 | SOCK_STREAM | SOCK_DGRAM) {
        return EAI_SOCKTYPE;
    }
    let protocol = hints.map_or(0, |hints| hints.protocol);
    if protocol != 0
        && !matches!(
            (socket_type, protocol),
            (0 | SOCK_STREAM, IPPROTO_TCP) | (0 | SOCK_DGRAM, IPPROTO_UDP)
        )
    {
        return EAI_SERVICE;
    }
    let port = match service_port(service.as_deref(), flags) {
        Ok(port) => port,
        Err(error) => return error,
    };
    let addresses: Vec<Ipv4Addr> = if let Some(node) = &node {
        if let Ok(address) = node.parse::<Ipv4Addr>() {
            vec![address]
        } else if flags & AI_NUMERICHOST != 0 {
            return EAI_NONAME;
        } else {
            match (node.as_str(), port).to_socket_addrs() {
                Ok(iter) => iter
                    .filter_map(|address| match address {
                        SocketAddr::V4(v4) => Some(*v4.ip()),
                        SocketAddr::V6(_) => None,
                    })
                    .collect(),
                Err(_) => return EAI_AGAIN,
            }
        }
    } else if flags & AI_PASSIVE != 0 {
        vec![Ipv4Addr::UNSPECIFIED]
    } else {
        vec![Ipv4Addr::LOCALHOST]
    };
    if addresses.is_empty() {
        return EAI_NONAME;
    }

    let mut head: *mut AddrInfo = std::ptr::null_mut();
    let mut tail: *mut AddrInfo = std::ptr::null_mut();
    for (index, address) in addresses.into_iter().enumerate() {
        let addr = crate::allocation::malloc(size_of::<SockAddrIn>()).cast::<SockAddrIn>();
        let info = crate::allocation::malloc(size_of::<AddrInfo>()).cast::<AddrInfo>();
        let name = if index == 0 && flags & AI_CANONNAME != 0 {
            // SAFETY: node is a Rust string without an embedded NUL.
            unsafe { allocate_c_string(node.as_deref().unwrap_or("")) }
        } else {
            std::ptr::null_mut()
        };
        if addr.is_null()
            || info.is_null()
            || index == 0 && flags & AI_CANONNAME != 0 && name.is_null()
        {
            // SAFETY: each pointer is either null or a fresh malloc result.
            unsafe {
                crate::allocation::free(addr.cast());
                crate::allocation::free(info.cast());
                crate::allocation::free(name.cast());
            }
            // SAFETY: head contains only nodes allocated by this function.
            unsafe { freeaddrinfo(head) };
            return EAI_MEMORY;
        }
        // SAFETY: both allocations are correctly sized and exclusively owned.
        unsafe {
            addr.write(SockAddrIn {
                family: AF_INET as u16,
                port: port.to_be(),
                address: address.octets(),
                padding: [0; 8],
            });
            info.write(AddrInfo {
                flags: 0,
                family: AF_INET,
                socket_type,
                protocol,
                address_length: size_of::<SockAddrIn>() as u32,
                address: addr,
                canonical_name: name,
                next: std::ptr::null_mut(),
            });
            if tail.is_null() {
                head = info;
            } else {
                (*tail).next = info;
            }
        }
        tail = info;
    }
    // SAFETY: result is an exclusive caller-provided output cell.
    unsafe { *result = head };
    0
}

/// # Safety
/// `head` must be a list returned by getaddrinfo, or NULL.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn freeaddrinfo(mut head: *mut AddrInfo) {
    while !head.is_null() {
        // SAFETY: each node owns its address, canonical name, and next link.
        let next = unsafe { (*head).next };
        unsafe {
            crate::allocation::free((*head).address.cast());
            crate::allocation::free((*head).canonical_name.cast());
            crate::allocation::free(head.cast());
        }
        head = next;
    }
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn gai_strerror(error: c_int) -> *const c_char {
    let message = match error {
        0 => c"Success",
        EAI_BADFLAGS => c"Invalid resolver flags",
        EAI_NONAME => c"Name or service not known",
        EAI_AGAIN => c"Temporary resolver failure",
        EAI_FAIL => c"Resolver failure",
        EAI_FAMILY => c"Address family unsupported",
        EAI_SOCKTYPE => c"Socket type unsupported",
        EAI_SERVICE => c"Service unsupported",
        EAI_MEMORY => c"Out of memory",
        EAI_OVERFLOW => c"Output buffer too small",
        _ => c"Unknown resolver error",
    };
    message.as_ptr()
}

/// # Safety
/// `address` must point to a readable sockaddr_in; non-null output buffers
/// must have their advertised capacities.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn getnameinfo(
    address: *const SockAddrIn,
    address_length: u32,
    host: *mut c_char,
    host_length: u32,
    service: *mut c_char,
    service_length: u32,
    flags: c_int,
) -> c_int {
    if address.is_null() || address_length < size_of::<SockAddrIn>() as u32 {
        return EAI_FAMILY;
    }
    if flags & !(NI_NUMERICHOST | NI_NUMERICSERV | NI_NOFQDN | NI_NAMEREQD | NI_DGRAM) != 0 {
        return EAI_BADFLAGS;
    }
    // SAFETY: caller supplied a full sockaddr_in.
    let address = unsafe { &*address };
    if address.family != AF_INET as u16 {
        return EAI_FAMILY;
    }
    if flags & NI_NAMEREQD != 0 && !host.is_null() {
        return EAI_NONAME;
    }
    let hostname = Ipv4Addr::from(address.address).to_string();
    let port = u16::from_be(address.port);
    let service_name = port.to_string();
    if !host.is_null() && hostname.len() >= host_length as usize
        || !service.is_null() && service_name.len() >= service_length as usize
    {
        return EAI_OVERFLOW;
    }
    // SAFETY: each output was checked for room including NUL.
    unsafe {
        if !host.is_null() {
            std::ptr::copy_nonoverlapping(hostname.as_ptr(), host.cast::<u8>(), hostname.len());
            *host.add(hostname.len()) = 0;
        }
        if !service.is_null() {
            std::ptr::copy_nonoverlapping(
                service_name.as_ptr(),
                service.cast::<u8>(),
                service_name.len(),
            );
            *service.add(service_name.len()) = 0;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn numeric_name_resolves_and_frees() {
        assert_eq!(
            unsafe { CStr::from_ptr(gai_strerror(EAI_NONAME)) }
                .to_str()
                .unwrap(),
            "Name or service not known"
        );
        let host = CString::new("127.0.0.1").unwrap();
        let service = CString::new("443").unwrap();
        let mut result = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                getaddrinfo(
                    host.as_ptr(),
                    service.as_ptr(),
                    std::ptr::null(),
                    &mut result,
                )
            },
            0
        );
        assert!(!result.is_null());
        let address = unsafe { &*(*result).address };
        assert_eq!(address.address, [127, 0, 0, 1]);
        assert_eq!(u16::from_be(address.port), 443);
        let mut host_text = [0i8; 16];
        let mut service_text = [0i8; 8];
        assert_eq!(
            unsafe {
                getnameinfo(
                    address,
                    16,
                    host_text.as_mut_ptr(),
                    16,
                    service_text.as_mut_ptr(),
                    8,
                    NI_NUMERICHOST | NI_NUMERICSERV,
                )
            },
            0
        );
        assert_eq!(
            unsafe { CStr::from_ptr(host_text.as_ptr()) }
                .to_str()
                .unwrap(),
            "127.0.0.1"
        );
        assert_eq!(
            unsafe { CStr::from_ptr(service_text.as_ptr()) }
                .to_str()
                .unwrap(),
            "443"
        );
        unsafe { freeaddrinfo(result) };
    }
}
