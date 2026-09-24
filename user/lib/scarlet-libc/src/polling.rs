//! POSIX poll over Scarlet Native handles. C descriptors are raw handles.

use std::ffi::{c_int, c_short};

use scarlet_abi::{ERRNO_EBADF, ERRNO_EINVAL, ERRNO_EIO, Syscall, fs::ERRNO_EFAULT};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PollFd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FdSet {
    bits: [u64; 16],
}

#[repr(C)]
pub struct Timeval {
    seconds: i64,
    microseconds: i64,
}

impl FdSet {
    fn contains(&self, fd: usize) -> bool {
        self.bits[fd / 64] & (1u64 << (fd % 64)) != 0
    }

    fn set(&mut self, fd: usize) {
        self.bits[fd / 64] |= 1u64 << (fd % 64);
    }
}

#[repr(C)]
struct PollOptions {
    timeout_ns: i64,
    min_timeout_ns: u64,
}

/// # Safety
/// `fds` must point to `nfds` readable and writable pollfd elements, unless
/// `nfds` is zero. The caller must not concurrently access the array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn poll(fds: *mut PollFd, nfds: usize, timeout_ms: c_int) -> c_int {
    if nfds > c_int::MAX as usize {
        return crate::fail(ERRNO_EINVAL);
    }
    if nfds != 0 && fds.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }

    // SAFETY: A zero-length slice never dereferences the caller's pointer;
    // otherwise the C contract provides an exclusive writable array.
    let originals: &mut [PollFd] = if nfds == 0 {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(fds, nfds) }
    };
    let mut active = Vec::new();
    if active.try_reserve_exact(nfds).is_err() {
        return crate::fail(scarlet_abi::fs::ERRNO_ENOMEM);
    }
    for entry in originals.iter_mut() {
        entry.revents = 0;
        // POSIX ignores negative descriptors. The Native Poll syscall reports
        // them as POLLNVAL, so filter them before entering the kernel.
        if entry.fd >= 0 {
            active.push(*entry);
        }
    }

    let options = PollOptions {
        timeout_ns: if timeout_ms < 0 {
            -1
        } else {
            i64::from(timeout_ms) * 1_000_000
        },
        min_timeout_ns: 0,
    };
    // SAFETY: PollFd has the Native PollHandle layout (u32,u16,u16); each
    // active descriptor is nonnegative. The kernel copies the array and
    // options synchronously, then writes only the revents field.
    let result = unsafe {
        scarlet_sys::syscall3(
            Syscall::Poll,
            if active.is_empty() {
                0
            } else {
                active.as_mut_ptr() as usize
            },
            active.len(),
            (&raw const options) as usize,
        )
    };
    if result == usize::MAX {
        return crate::fail(ERRNO_EIO);
    }

    let mut ready = 0;
    let mut current = active.iter();
    for entry in originals.iter_mut().filter(|entry| entry.fd >= 0) {
        entry.revents = current.next().expect("active count matches input").revents;
        ready += usize::from(entry.revents != 0);
    }
    ready as c_int
}

/// # Safety
/// Non-null descriptor sets and timeout must be exclusively writable for this
/// call. `nfds` bounds every bit inspected in the supplied sets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn select(
    nfds: c_int,
    read: *mut FdSet,
    write: *mut FdSet,
    except: *mut FdSet,
    timeout: *mut Timeval,
) -> c_int {
    if !(0..=1024).contains(&nfds) {
        return crate::fail(ERRNO_EINVAL);
    }
    let timeout_ms = if timeout.is_null() {
        -1
    } else {
        // SAFETY: caller supplies a readable timeval.
        let timeout = unsafe { &*timeout };
        if timeout.seconds < 0 || !(0..1_000_000).contains(&timeout.microseconds) {
            return crate::fail(ERRNO_EINVAL);
        }
        let milliseconds =
            timeout.seconds.saturating_mul(1000) + (timeout.microseconds + 999) / 1000;
        milliseconds.min(c_int::MAX as i64) as c_int
    };
    // SAFETY: each non-null set is exclusively borrowed for the syscall.
    let requested_read = unsafe { read.as_ref().copied() };
    let requested_write = unsafe { write.as_ref().copied() };
    let requested_except = unsafe { except.as_ref().copied() };
    let mut descriptors = Vec::new();
    if descriptors.try_reserve_exact(nfds as usize).is_err() {
        return crate::fail(scarlet_abi::fs::ERRNO_ENOMEM);
    }
    for fd in 0..nfds as usize {
        let mut events = 0;
        if requested_read.as_ref().is_some_and(|set| set.contains(fd)) {
            events |= 0x0001;
        }
        if requested_write.as_ref().is_some_and(|set| set.contains(fd)) {
            events |= 0x0004;
        }
        if requested_except
            .as_ref()
            .is_some_and(|set| set.contains(fd))
        {
            events |= 0x0002;
        }
        if events != 0 {
            descriptors.push(PollFd {
                fd: fd as c_int,
                events,
                revents: 0,
            });
        }
    }
    // SAFETY: the Vec exposes its initialized PollFd elements exclusively.
    let result = unsafe { poll(descriptors.as_mut_ptr(), descriptors.len(), timeout_ms) };
    if result < 0 {
        return -1;
    }
    if descriptors.iter().any(|entry| entry.revents & 0x0020 != 0) {
        return crate::fail(ERRNO_EBADF);
    }
    let mut ready_read = FdSet { bits: [0; 16] };
    let mut ready_write = FdSet { bits: [0; 16] };
    let mut ready_except = FdSet { bits: [0; 16] };
    let mut count = 0;
    for entry in &descriptors {
        let fd = entry.fd as usize;
        if requested_read.as_ref().is_some_and(|set| set.contains(fd))
            && entry.revents & (0x0001 | 0x0008 | 0x0010) != 0
        {
            ready_read.set(fd);
            count += 1;
        }
        if requested_write.as_ref().is_some_and(|set| set.contains(fd))
            && entry.revents & (0x0004 | 0x0008) != 0
        {
            ready_write.set(fd);
            count += 1;
        }
        if requested_except
            .as_ref()
            .is_some_and(|set| set.contains(fd))
            && entry.revents & 0x0002 != 0
        {
            ready_except.set(fd);
            count += 1;
        }
    }
    // SAFETY: caller grants exclusive writable access to non-null sets.
    unsafe {
        if !read.is_null() {
            *read = ready_read;
        }
        if !write.is_null() {
            *write = ready_write;
        }
        if !except.is_null() {
            *except = ready_except;
        }
    }
    count
}
