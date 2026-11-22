//! A preallocated Mach semaphore bridges the realtime render callback to the
//! IRQ relay. A signal posted before the relay waits is retained by the kernel.
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

use libc::{kern_return_t, mach_port_t};

unsafe extern "C" {
    static mach_task_self_: mach_port_t;
    fn semaphore_create(
        task: mach_port_t,
        sem: *mut mach_port_t,
        policy: i32,
        value: i32,
    ) -> kern_return_t;
    fn semaphore_destroy(task: mach_port_t, sem: mach_port_t) -> kern_return_t;
    fn semaphore_signal(sem: mach_port_t) -> kern_return_t;
    fn semaphore_wait(sem: mach_port_t) -> kern_return_t;
}

pub(super) struct CompletionSignal {
    semaphore: mach_port_t,
    pending: AtomicBool,
    closed: AtomicBool,
}

impl CompletionSignal {
    pub fn new() -> io::Result<Self> {
        const SYNC_POLICY_FIFO: i32 = 0;
        let mut semaphore = 0;
        let status =
            unsafe { semaphore_create(mach_task_self_, &mut semaphore, SYNC_POLICY_FIFO, 0) };
        if status != libc::KERN_SUCCESS {
            return Err(io::Error::other(format!(
                "Cannot create audio completion semaphore: {status}"
            )));
        }
        Ok(Self {
            semaphore,
            pending: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    pub fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            // Non-waiting signal on a live port retained by the render queue.
            unsafe { semaphore_signal(self.semaphore) };
        }
    }

    pub fn wait(&self) -> bool {
        loop {
            let status = unsafe { semaphore_wait(self.semaphore) };
            if status != libc::KERN_ABORTED {
                assert_eq!(status, libc::KERN_SUCCESS, "audio completion wait failed");
                break;
            }
        }
        self.pending.swap(false, Ordering::AcqRel);
        !self.closed.load(Ordering::Acquire)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        // Wake even if the relay is currently dispatching the last completion.
        unsafe { semaphore_signal(self.semaphore) };
    }

    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}

impl Drop for CompletionSignal {
    fn drop(&mut self) {
        unsafe { semaphore_destroy(mach_task_self_, self.semaphore) };
    }
}
