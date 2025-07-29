use crate::{syscall::{syscall1, Syscall}};
use core::time::Duration;

pub fn sleep(dur: Duration) -> i32 {
    let nanosecs = dur.as_nanos() as usize;
    syscall1(Syscall::Sleep, nanosecs) as i32
}