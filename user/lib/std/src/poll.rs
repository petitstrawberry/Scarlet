use crate::syscall::{Syscall, syscall3};

pub const POLLIN: u16 = 0x0001;
pub const POLLPRI: u16 = 0x0002;
pub const POLLOUT: u16 = 0x0004;
pub const POLLERR: u16 = 0x0008;
pub const POLLHUP: u16 = 0x0010;
pub const POLLNVAL: u16 = 0x0020;

#[repr(C)]
pub struct PollHandle {
    pub handle: u32,
    pub events: u16,
    pub revents: u16,
}

impl PollHandle {
    pub fn new(handle: u32, events: u16) -> Self {
        Self {
            handle,
            events,
            revents: 0,
        }
    }
}

#[repr(C)]
pub struct PollOptions {
    pub timeout_ns: i64,
    pub min_timeout_ns: u64,
}

impl PollOptions {
    pub const fn new() -> Self {
        Self {
            timeout_ns: 0,
            min_timeout_ns: 0,
        }
    }

    pub const fn timeout(mut self, ns: i64) -> Self {
        self.timeout_ns = ns;
        self
    }

    pub const fn min_timeout(mut self, ns: u64) -> Self {
        self.min_timeout_ns = ns;
        self
    }
}

pub fn poll(handles: &mut [PollHandle], timeout_ns: i64) -> Result<usize, i32> {
    let opts = PollOptions::new().timeout(timeout_ns);
    poll_with_options(handles, &opts)
}

pub fn poll_with_options(handles: &mut [PollHandle], options: &PollOptions) -> Result<usize, i32> {
    let res = syscall3(
        Syscall::Poll,
        handles.as_mut_ptr() as usize,
        handles.len(),
        options as *const PollOptions as usize,
    );
    if res == usize::MAX { Err(-1) } else { Ok(res) }
}
