use crate::syscall::{Syscall, syscall1};
use core::time::Duration;

pub fn sleep(dur: Duration) -> i32 {
    let nanosecs = dur.as_nanos() as usize;
    syscall1(Syscall::Sleep, nanosecs) as i32
}
