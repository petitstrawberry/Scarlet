//! POSIX IPv4 socket adaptation over Scarlet Native socket handles.
//!
//! Unsupported flags and options return errors. In particular, there is no
//! emulation of `MSG_PEEK` or IPv6 through an IPv4 Native socket.

use std::ffi::{c_int, c_void};

use scarlet_abi::{
    ERRNO_EBADF, ERRNO_EINVAL, ERRNO_EIO, ERRNO_EOPNOTSUPP, Syscall, fs::ERRNO_EFAULT,
};

const AF_UNIX: c_int = 1;
const AF_INET: c_int = 2;
const AF_INET6: c_int = 10;
const SOCK_STREAM: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const SOCK_RAW: c_int = 3;
const SOCK_SEQPACKET: c_int = 5;
const SOCK_NONBLOCK: c_int = 0x800;
const SOCK_CLOEXEC: c_int = 0x80000;
const MSG_NOSIGNAL: c_int = 0x4000;
const SOL_SOCKET: c_int = 1;
const SO_ERROR: c_int = 4;
const SO_RCVTIMEO: c_int = 20;
const SO_SNDTIMEO: c_int = 21;
const EAFNOSUPPORT: c_int = 97;

#[repr(C)]
pub struct SockAddr {
    family: u16,
    data: [u8; 14],
}

#[repr(C)]
struct SockAddrIn {
    family: u16,
    port: u16,
    address: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
struct NativeInet4Address {
    address: [u8; 4],
    port: u16,
}

#[repr(C)]
struct Timeval {
    seconds: i64,
    microseconds: i64,
}

fn syscall_result(result: usize) -> Result<usize, c_int> {
    if result == usize::MAX {
        Err(ERRNO_EIO)
    } else if result > isize::MAX as usize {
        Err((result as isize).wrapping_neg() as c_int)
    } else {
        Ok(result)
    }
}

fn status(result: usize) -> c_int {
    match syscall_result(result) {
        Ok(_) => 0,
        Err(error) => crate::fail(error),
    }
}

fn valid_fd(fd: c_int) -> Result<usize, c_int> {
    if fd < 0 {
        Err(ERRNO_EBADF)
    } else {
        Ok(fd as usize)
    }
}

/// # Safety
/// `address` must point to a readable sockaddr with at least `length` bytes.
unsafe fn inet4_address(
    address: *const SockAddr,
    length: u32,
) -> Result<NativeInet4Address, c_int> {
    if address.is_null() {
        return Err(ERRNO_EFAULT);
    }
    if length < size_of::<SockAddrIn>() as u32 {
        return Err(ERRNO_EINVAL);
    }
    // SAFETY: the caller provides at least one full sockaddr_in.
    let input = unsafe { &*address.cast::<SockAddrIn>() };
    if input.family != AF_INET as u16 {
        return Err(EAFNOSUPPORT);
    }
    Ok(NativeInet4Address {
        address: input.address,
        port: u16::from_be(input.port),
    })
}

fn close_handle(handle: usize) {
    // SAFETY: Native close consumes an integer handle, never a userspace pointer.
    unsafe { scarlet_sys::syscall1(Syscall::HandleClose, handle) };
}

#[unsafe(no_mangle)]
pub extern "C" fn socket(domain: c_int, socket_type: c_int, protocol: c_int) -> c_int {
    let native_domain = match domain {
        AF_UNIX => 1,
        AF_INET => 2,
        AF_INET6 => return crate::fail(EAFNOSUPPORT),
        _ => return crate::fail(EAFNOSUPPORT),
    };
    let native_type = match socket_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC) {
        SOCK_STREAM => 1,
        SOCK_DGRAM => 2,
        SOCK_RAW => 3,
        SOCK_SEQPACKET => 4,
        _ => return crate::fail(ERRNO_EOPNOTSUPP),
    };
    if protocol < 0 {
        return crate::fail(ERRNO_EINVAL);
    }
    // SAFETY: SocketCreate consumes only validated scalar arguments.
    let handle = unsafe {
        scarlet_sys::syscall3(
            Syscall::SocketCreate,
            native_domain,
            native_type,
            protocol as usize,
        )
    };
    if handle == usize::MAX {
        return crate::fail(ERRNO_EIO);
    }
    if handle > c_int::MAX as usize {
        close_handle(handle);
        return crate::fail(ERRNO_EIO);
    }
    if socket_type & SOCK_CLOEXEC != 0 {
        // SAFETY: close-on-exec belongs to this new descriptor.
        let result = unsafe { scarlet_sys::syscall2(Syscall::HandleSetDescriptorFlags, handle, 1) };
        if let Err(error) = syscall_result(result) {
            close_handle(handle);
            return crate::fail(error);
        }
    }
    if socket_type & SOCK_NONBLOCK != 0 {
        // SAFETY: the handle was just created and is owned by the caller.
        let result = unsafe {
            scarlet_sys::syscall3(
                Syscall::HandleControl,
                handle,
                scarlet_abi::SCTL_SOCKET_SET_NONBLOCK as usize,
                1,
            )
        };
        if let Err(error) = syscall_result(result) {
            close_handle(handle);
            return crate::fail(error);
        }
    }
    handle as c_int
}

/// # Safety
/// `address` must point to a readable sockaddr with `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bind(fd: c_int, address: *const SockAddr, length: u32) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    let native = match unsafe { inet4_address(address, length) } {
        Ok(native) => native,
        Err(error) => return crate::fail(error),
    };
    // SAFETY: kernel copies the short Native IPv4 address synchronously.
    status(unsafe {
        scarlet_sys::syscall3(
            Syscall::SocketBind,
            handle,
            (&raw const native) as usize,
            size_of::<NativeInet4Address>(),
        )
    })
}

/// # Safety
/// `address` must point to a readable sockaddr with `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn connect(fd: c_int, address: *const SockAddr, length: u32) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    let native = match unsafe { inet4_address(address, length) } {
        Ok(native) => native,
        Err(error) => return crate::fail(error),
    };
    // SAFETY: kernel copies the short Native IPv4 address synchronously.
    status(unsafe {
        scarlet_sys::syscall3(
            Syscall::SocketConnect,
            handle,
            (&raw const native) as usize,
            size_of::<NativeInet4Address>(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn listen(fd: c_int, backlog: c_int) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    if backlog < 0 {
        return crate::fail(ERRNO_EINVAL);
    }
    // SAFETY: only scalar arguments are passed.
    status(unsafe { scarlet_sys::syscall2(Syscall::SocketListen, handle, backlog as usize) })
}

/// # Safety
/// When non-null, `address` and `length` must be exclusively writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn accept(fd: c_int, address: *mut SockAddr, length: *mut u32) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    if !address.is_null() && length.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    // SAFETY: Native accept returns a new owned handle.
    let accepted =
        match syscall_result(unsafe { scarlet_sys::syscall1(Syscall::SocketAccept, handle) }) {
            Ok(accepted) => accepted,
            Err(error) => return crate::fail(error),
        };
    if accepted > c_int::MAX as usize {
        close_handle(accepted);
        return crate::fail(ERRNO_EIO);
    }
    if !address.is_null() {
        // SAFETY: pointer validity and capacity are covered by the C contract.
        if unsafe { socket_name(accepted, address, length, true) }.is_err() {
            close_handle(accepted);
            return -1;
        }
    }
    accepted as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn shutdown(fd: c_int, how: c_int) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    if !(0..=2).contains(&how) {
        return crate::fail(ERRNO_EINVAL);
    }
    // SAFETY: only scalar arguments are passed.
    status(unsafe { scarlet_sys::syscall2(Syscall::SocketShutdown, handle, how as usize) })
}

fn valid_message(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
) -> Result<usize, c_int> {
    let handle = valid_fd(fd)?;
    if length > isize::MAX as usize {
        return Err(ERRNO_EINVAL);
    }
    if buffer.is_null() && length != 0 {
        return Err(ERRNO_EFAULT);
    }
    if flags & !MSG_NOSIGNAL != 0 {
        return Err(ERRNO_EOPNOTSUPP);
    }
    Ok(handle)
}

/// # Safety
/// `buffer` must point to `length` readable bytes unless length is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn send(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
) -> isize {
    let handle = match valid_message(fd, buffer, length, flags) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error) as isize,
    };
    // SAFETY: the kernel copies the caller's buffer synchronously.
    match syscall_result(unsafe {
        scarlet_sys::syscall3(Syscall::StreamWrite, handle, buffer as usize, length)
    }) {
        Ok(written) if written <= length => written as isize,
        Ok(_) => crate::fail(ERRNO_EIO) as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

/// # Safety
/// `buffer` must point to `length` writable bytes unless length is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn recv(
    fd: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
) -> isize {
    let handle = match valid_message(fd, buffer, length, flags) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error) as isize,
    };
    // SAFETY: the kernel writes at most length bytes to the caller's buffer.
    match syscall_result(unsafe {
        scarlet_sys::syscall3(Syscall::StreamRead, handle, buffer as usize, length)
    }) {
        Ok(received) if received <= length => received as isize,
        Ok(_) => crate::fail(ERRNO_EIO) as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

/// # Safety
/// `buffer` and `address` must be readable for their advertised lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sendto(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
    address: *const SockAddr,
    address_length: u32,
) -> isize {
    let handle = match valid_message(fd, buffer, length, flags) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error) as isize,
    };
    let native = match unsafe { inet4_address(address, address_length) } {
        Ok(native) => native,
        Err(error) => return crate::fail(error) as isize,
    };
    let mut raw = [
        2,
        0,
        native.address[0],
        native.address[1],
        native.address[2],
        native.address[3],
        0,
        0,
    ];
    raw[6..8].copy_from_slice(&native.port.to_be_bytes());
    // SAFETY: the kernel copies both buffers synchronously.
    match syscall_result(unsafe {
        scarlet_sys::syscall4(
            Syscall::SocketSendTo,
            handle,
            buffer as usize,
            length,
            raw.as_ptr() as usize,
        )
    }) {
        Ok(written) if written <= length => written as isize,
        Ok(_) => crate::fail(ERRNO_EIO) as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

/// # Safety
/// `buffer` and non-null address outputs must be exclusively writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn recvfrom(
    fd: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    address: *mut SockAddr,
    address_length: *mut u32,
) -> isize {
    let handle = match valid_message(fd, buffer, length, flags) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error) as isize,
    };
    if !address.is_null() && address_length.is_null() {
        return crate::fail(ERRNO_EFAULT) as isize;
    }
    let mut raw = [0u8; 8];
    // SAFETY: kernel writes at most length bytes and exactly eight address bytes.
    let received = match syscall_result(unsafe {
        scarlet_sys::syscall4(
            Syscall::SocketRecvFrom,
            handle,
            buffer as usize,
            length,
            raw.as_mut_ptr() as usize,
        )
    }) {
        Ok(received) if received <= length => received,
        Ok(_) => return crate::fail(ERRNO_EIO) as isize,
        Err(error) => return crate::fail(error) as isize,
    };
    if !address.is_null() {
        // SAFETY: the caller advertises the address buffer's capacity.
        unsafe { write_sockaddr(&raw, address, address_length) };
    }
    received as isize
}

/// # Safety
/// `address` and `length` must be exclusively writable.
unsafe fn write_sockaddr(raw: &[u8; 8], address: *mut SockAddr, length: *mut u32) {
    let mut posix = SockAddrIn {
        family: AF_INET as u16,
        port: u16::from_be_bytes([raw[6], raw[7]]).to_be(),
        address: [raw[2], raw[3], raw[4], raw[5]],
        zero: [0; 8],
    };
    // SAFETY: the caller supplies a writable length and buffer of that size.
    let capacity = unsafe { *length } as usize;
    let write_length = capacity.min(size_of::<SockAddrIn>());
    unsafe {
        std::ptr::copy_nonoverlapping(
            (&raw mut posix).cast::<u8>(),
            address.cast::<u8>(),
            write_length,
        );
        *length = size_of::<SockAddrIn>() as u32;
    }
}

/// # Safety
/// `address` and `length` must be exclusively writable.
unsafe fn socket_name(
    handle: usize,
    address: *mut SockAddr,
    length: *mut u32,
    peer: bool,
) -> Result<(), ()> {
    if address.is_null() || length.is_null() {
        crate::fail(ERRNO_EFAULT);
        return Err(());
    }
    let mut raw = [0u8; 8];
    let call = if peer {
        Syscall::SocketGetPeerAddress
    } else {
        Syscall::SocketGetLocalAddress
    };
    // SAFETY: Native address query writes exactly eight bytes.
    if let Err(error) =
        syscall_result(unsafe { scarlet_sys::syscall2(call, handle, raw.as_mut_ptr() as usize) })
    {
        crate::fail(error);
        return Err(());
    }
    if raw[0] != AF_INET as u8 {
        crate::fail(ERRNO_EIO);
        return Err(());
    }
    unsafe { write_sockaddr(&raw, address, length) };
    Ok(())
}

/// # Safety
/// `address` and `length` must be exclusively writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getsockname(fd: c_int, address: *mut SockAddr, length: *mut u32) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    unsafe { socket_name(handle, address, length, false) }.map_or(-1, |()| 0)
}

/// # Safety
/// `address` and `length` must be exclusively writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getpeername(fd: c_int, address: *mut SockAddr, length: *mut u32) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    unsafe { socket_name(handle, address, length, true) }.map_or(-1, |()| 0)
}

/// # Safety
/// `value` and `length` must be exclusively writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getsockopt(
    fd: c_int,
    level: c_int,
    option: c_int,
    value: *mut c_void,
    length: *mut u32,
) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    if value.is_null() || length.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    if level != SOL_SOCKET || option != SO_ERROR {
        return crate::fail(ERRNO_EOPNOTSUPP);
    }
    if unsafe { *length } < size_of::<c_int>() as u32 {
        return crate::fail(ERRNO_EINVAL);
    }
    // SAFETY: the control operation returns and clears one pending error.
    let result = unsafe {
        scarlet_sys::syscall3(
            Syscall::HandleControl,
            handle,
            scarlet_abi::SCTL_SOCKET_TAKE_ERROR as usize,
            0,
        )
    };
    let error = match syscall_result(result) {
        Ok(error) if error <= c_int::MAX as usize => error as c_int,
        Ok(_) => return crate::fail(ERRNO_EIO),
        Err(error) => return crate::fail(error),
    };
    // SAFETY: caller supplied room for one C int.
    unsafe {
        *value.cast::<c_int>() = error;
        *length = size_of::<c_int>() as u32;
    }
    0
}

/// # Safety
/// `value` must point to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn setsockopt(
    fd: c_int,
    level: c_int,
    option: c_int,
    value: *const c_void,
    length: u32,
) -> c_int {
    let handle = match valid_fd(fd) {
        Ok(handle) => handle,
        Err(error) => return crate::fail(error),
    };
    if value.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    if level != SOL_SOCKET || !matches!(option, SO_RCVTIMEO | SO_SNDTIMEO) {
        return crate::fail(ERRNO_EOPNOTSUPP);
    }
    if length < size_of::<Timeval>() as u32 {
        return crate::fail(ERRNO_EINVAL);
    }
    // SAFETY: caller provided a full timeval.
    let timeout = unsafe { &*value.cast::<Timeval>() };
    if timeout.seconds < 0 || !(0..1_000_000).contains(&timeout.microseconds) {
        return crate::fail(ERRNO_EINVAL);
    }
    let millis = match timeout
        .seconds
        .checked_mul(1000)
        .and_then(|v| v.checked_add((timeout.microseconds + 999) / 1000))
    {
        Some(millis) => millis as usize,
        None => return crate::fail(ERRNO_EINVAL),
    };
    let control = if option == SO_RCVTIMEO {
        scarlet_abi::SCTL_SOCKET_SET_READ_TIMEOUT_MS
    } else {
        scarlet_abi::SCTL_SOCKET_SET_WRITE_TIMEOUT_MS
    };
    // SAFETY: scalar timeout copied from the caller's timeval.
    status(unsafe {
        scarlet_sys::syscall3(Syscall::HandleControl, handle, control as usize, millis)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn socketpair(
    _domain: c_int,
    _socket_type: c_int,
    _protocol: c_int,
    _fds: *mut c_int,
) -> c_int {
    crate::fail(ERRNO_EOPNOTSUPP)
}
