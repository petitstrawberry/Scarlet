//! IPv4/IPv6 text conversion for the POSIX network API.

use std::cell::UnsafeCell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::net::{Ipv4Addr, Ipv6Addr};

const AF_INET: c_int = 2;
const AF_INET6: c_int = 10;
const EAFNOSUPPORT: c_int = 97;

thread_local! {
    static NTOA_BUFFER: UnsafeCell<[c_char; 16]> = const { UnsafeCell::new([0; 16]) };
}

/// # Safety
/// `source` is a NUL-terminated string and `destination` points to at least
/// four bytes for IPv4 or sixteen bytes for IPv6.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn inet_pton(
    family: c_int,
    source: *const c_char,
    destination: *mut c_void,
) -> c_int {
    if source.is_null() || destination.is_null() {
        return crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
    }
    // SAFETY: caller supplies a NUL-terminated source string.
    let source = unsafe { CStr::from_ptr(source) };
    let Ok(source) = source.to_str() else {
        return 0;
    };
    match family {
        AF_INET => match source.parse::<Ipv4Addr>() {
            Ok(address) => {
                // SAFETY: the caller supplies at least four writable bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        address.octets().as_ptr(),
                        destination.cast::<u8>(),
                        4,
                    )
                };
                1
            }
            Err(_) => 0,
        },
        AF_INET6 => match source.parse::<Ipv6Addr>() {
            Ok(address) => {
                // SAFETY: the caller supplies at least sixteen writable bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        address.octets().as_ptr(),
                        destination.cast::<u8>(),
                        16,
                    )
                };
                1
            }
            Err(_) => 0,
        },
        _ => crate::fail(EAFNOSUPPORT),
    }
}

/// # Safety
/// `source` points to at least four bytes for IPv4 or sixteen bytes for IPv6;
/// `destination` points to `size` writable bytes.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn inet_ntop(
    family: c_int,
    source: *const c_void,
    destination: *mut c_char,
    size: u32,
) -> *const c_char {
    if source.is_null() || destination.is_null() {
        crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
        return std::ptr::null();
    }
    let formatted = match family {
        AF_INET => {
            let mut octets = [0u8; 4];
            // SAFETY: the caller supplies at least four readable bytes.
            unsafe { std::ptr::copy_nonoverlapping(source.cast::<u8>(), octets.as_mut_ptr(), 4) };
            Ipv4Addr::from(octets).to_string()
        }
        AF_INET6 => {
            let mut octets = [0u8; 16];
            // SAFETY: the caller supplies at least sixteen readable bytes.
            unsafe { std::ptr::copy_nonoverlapping(source.cast::<u8>(), octets.as_mut_ptr(), 16) };
            Ipv6Addr::from(octets).to_string()
        }
        _ => {
            crate::fail(EAFNOSUPPORT);
            return std::ptr::null();
        }
    };
    if formatted.len() >= size as usize {
        crate::fail(scarlet_abi::fs::ERRNO_ENOSPC);
        return std::ptr::null();
    }
    // SAFETY: caller supplies `size` bytes; the check above includes NUL.
    unsafe {
        std::ptr::copy_nonoverlapping(
            formatted.as_ptr(),
            destination.cast::<u8>(),
            formatted.len(),
        );
        *destination.add(formatted.len()) = 0;
    }
    destination
}

/// # Safety
/// `source` must be a NUL-terminated string.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn inet_addr(source: *const c_char) -> u32 {
    if source.is_null() {
        crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
        return u32::MAX;
    }
    // SAFETY: caller supplies a NUL-terminated source string.
    let Ok(source) = unsafe { CStr::from_ptr(source) }.to_str() else {
        return u32::MAX;
    };
    match source.parse::<Ipv4Addr>() {
        Ok(address) => u32::from_ne_bytes(address.octets()),
        Err(_) => u32::MAX,
    }
}

/// # Safety
/// The returned pointer is valid until this thread next calls inet_ntoa.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn inet_ntoa(address: u32) -> *mut c_char {
    let octets = address.to_ne_bytes();
    let formatted = Ipv4Addr::from(octets).to_string();
    NTOA_BUFFER.with(|buffer| {
        // SAFETY: this thread exclusively owns its TLS buffer; an IPv4 string
        // needs at most fifteen bytes plus NUL.
        let buffer = unsafe { &mut *buffer.get() };
        for (slot, byte) in buffer.iter_mut().zip(formatted.bytes()) {
            *slot = byte as c_char;
        }
        buffer[formatted.len()] = 0;
        buffer.as_mut_ptr()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn ipv4_round_trip_has_network_order_bytes() {
        let input = CString::new("127.0.0.1").unwrap();
        let mut bytes = [0u8; 4];
        assert_eq!(
            unsafe { inet_pton(AF_INET, input.as_ptr(), bytes.as_mut_ptr().cast()) },
            1
        );
        assert_eq!(bytes, [127, 0, 0, 1]);
        assert_eq!(unsafe { inet_addr(input.as_ptr()) }.to_ne_bytes(), bytes);
        let text = unsafe { CStr::from_ptr(inet_ntoa(u32::from_ne_bytes(bytes))) };
        assert_eq!(text.to_str().unwrap(), "127.0.0.1");
        let mut output = [0i8; 16];
        let returned = unsafe {
            inet_ntop(
                AF_INET,
                bytes.as_ptr().cast(),
                output.as_mut_ptr(),
                output.len() as u32,
            )
        };
        assert_eq!(returned, output.as_ptr());
        assert_eq!(
            unsafe { CStr::from_ptr(returned) }.to_str().unwrap(),
            "127.0.0.1"
        );
    }

    #[test]
    fn invalid_input_preserves_destination() {
        let input = CString::new("not.an.address").unwrap();
        let mut bytes = [0x5au8; 16];
        assert_eq!(
            unsafe { inet_pton(AF_INET6, input.as_ptr(), bytes.as_mut_ptr().cast()) },
            0
        );
        assert_eq!(bytes, [0x5a; 16]);
    }
}
