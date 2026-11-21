//! AAPCS64 kernel continuations and their guarded host stacks.

use std::{arch::naked_asm, ffi::c_void, io, ptr::NonNull};

/// A saved host continuation. Guest registers and TLS belong to the worker.
#[repr(C, align(16))]
#[derive(Default)]
pub struct Registers {
    x19_through_x30: [usize; 12],
    sp: usize,
    fpcr: u64,
    fpsr: u64,
    simd: [u128; 8],
}

const _: () = {
    assert!(std::mem::offset_of!(Registers, sp) == 96);
    assert!(std::mem::offset_of!(Registers, fpcr) == 104);
    assert!(std::mem::offset_of!(Registers, fpsr) == 112);
    assert!(std::mem::offset_of!(Registers, simd) == 128);
};

pub type TaskEntry = unsafe extern "C" fn(*mut c_void) -> !;

/// A suspended or unstarted task. The register address and stack remain
/// stable across moves, including transfers between host vCPU threads.
pub struct TaskContext {
    registers: Box<Registers>,
    _stack: GuardedStack,
}

impl TaskContext {
    /// Build a fresh continuation that enters `entry(argument)` on first resume.
    ///
    /// The caller supplies the argument's lifetime when switching into this
    /// context. The entry must keep the context alive until it switches out.
    pub fn new(stack_size: usize, entry: TaskEntry, argument: *mut c_void) -> io::Result<Self> {
        let stack = GuardedStack::new(stack_size)?;
        let mut registers = Box::new(Registers {
            sp: stack.top(),
            ..Registers::default()
        });
        registers.x19_through_x30[0] = argument.expose_provenance();
        registers.x19_through_x30[1] = entry as *const () as usize;
        registers.x19_through_x30[11] = enter_task as *const () as usize;
        Ok(Self {
            registers,
            _stack: stack,
        })
    }

    pub fn registers_ptr(&mut self) -> *mut Registers {
        &raw mut *self.registers
    }

    pub fn registers(&self) -> *const Registers {
        &raw const *self.registers
    }

    pub fn stack_top(&self) -> usize {
        self._stack.top()
    }
}

struct GuardedStack {
    base: NonNull<c_void>,
    mapping_size: usize,
    guard_size: usize,
}

// The mapping is owned by the stack and can be unmapped from any host thread.
// The scheduler separately owns the right to run the saved continuation.
unsafe impl Send for GuardedStack {}

impl GuardedStack {
    fn new(size: usize) -> io::Result<Self> {
        // SAFETY: sysconf has no pointer arguments.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if size == 0 || page_size <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Stack and host page sizes must be positive",
            ));
        }
        let guard_size = page_size as usize;
        let stack_size = size
            .checked_next_multiple_of(guard_size)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let mapping_size = stack_size
            .checked_add(2 * guard_size)
            .ok_or(io::ErrorKind::InvalidInput)?;

        // SAFETY: A fresh anonymous mapping has no file or fixed address.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mapping_size,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let stack = Self {
            base: NonNull::new(base).expect("macOS returned a null stack mapping"),
            mapping_size,
            guard_size,
        };
        // SAFETY: The middle range is page-aligned and lies within our mapping.
        let result = unsafe {
            libc::mprotect(
                base.byte_add(guard_size),
                stack_size,
                libc::PROT_READ | libc::PROT_WRITE,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(stack)
    }

    fn top(&self) -> usize {
        self.base.as_ptr().addr() + self.mapping_size - self.guard_size
    }
}

impl Drop for GuardedStack {
    fn drop(&mut self) {
        // SAFETY: The context owner has stopped executing on this stack.
        unsafe { libc::munmap(self.base.as_ptr(), self.mapping_size) };
    }
}

#[unsafe(naked)]
#[unsafe(export_name = "scarlet_hosted_enter_task")]
unsafe extern "C" fn enter_task() -> ! {
    naked_asm!("mov x0, x19", "br x20",);
}

/// Suspend the active kernel continuation and resume a stopped one.
///
/// # Safety
///
/// Both register slots and their stack storage must remain valid until their
/// continuations retire. The pointers must be distinct and exclusively owned
/// for the duration of the transfer. `next` must be stopped and initialized
/// by a prior switch or by `TaskContext::new`. Initial arguments must remain
/// valid for entry. Callers must release thread-affine host state before
/// allowing a continuation to move to another host thread.
#[unsafe(naked)]
#[unsafe(export_name = "scarlet_hosted_switch_context")]
pub unsafe extern "C" fn switch(previous: *mut Registers, next: *const Registers) {
    naked_asm!(
        "stp x19, x20, [x0, #0]",
        "stp x21, x22, [x0, #16]",
        "stp x23, x24, [x0, #32]",
        "stp x25, x26, [x0, #48]",
        "stp x27, x28, [x0, #64]",
        "stp x29, x30, [x0, #80]",
        "mov x9, sp",
        "mrs x10, fpcr",
        "mrs x11, fpsr",
        "stp x9, x10, [x0, #96]",
        "str x11, [x0, #112]",
        "stp q8, q9, [x0, #128]",
        "stp q10, q11, [x0, #160]",
        "stp q12, q13, [x0, #192]",
        "stp q14, q15, [x0, #224]",
        "ldp x19, x20, [x1, #0]",
        "ldp x21, x22, [x1, #16]",
        "ldp x23, x24, [x1, #32]",
        "ldp x25, x26, [x1, #48]",
        "ldp x27, x28, [x1, #64]",
        "ldp x29, x30, [x1, #80]",
        "ldp x9, x10, [x1, #96]",
        "ldr x11, [x1, #112]",
        "ldp q8, q9, [x1, #128]",
        "ldp q10, q11, [x1, #160]",
        "ldp q12, q13, [x1, #192]",
        "ldp q14, q15, [x1, #224]",
        "msr fpcr, x10",
        "msr fpsr, x11",
        "mov sp, x9",
        "ret",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc, Barrier,
            atomic::{AtomicPtr, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
    };

    #[derive(Default)]
    struct Exchange {
        task: AtomicPtr<Registers>,
        scheduler: AtomicPtr<Registers>,
        first_thread: AtomicUsize,
        resumed_thread: AtomicUsize,
        resumed_local: AtomicUsize,
        initial_cpu: AtomicUsize,
        resumed_cpu: AtomicUsize,
    }

    unsafe extern "C" fn task(argument: *mut c_void) -> ! {
        // SAFETY: Both scheduler threads retain the heap-owned exchange.
        let exchange = unsafe { &*argument.cast::<Exchange>() };
        let local = std::hint::black_box(0x1234usize);
        exchange
            .initial_cpu
            .store(crate::cpu_binding::current().unwrap(), Ordering::Relaxed);
        // SAFETY: pthread_self reads the current host thread identity.
        exchange
            .first_thread
            .store(unsafe { libc::pthread_self() }, Ordering::Relaxed);
        // SAFETY: The current vCPU published its stopped scheduler registers
        // before switching into the task. The task owns its running slot.
        unsafe {
            switch(
                exchange.task.load(Ordering::Relaxed),
                exchange.scheduler.load(Ordering::Relaxed),
            )
        };

        exchange
            .resumed_thread
            .store(unsafe { libc::pthread_self() }, Ordering::Relaxed);
        exchange
            .resumed_cpu
            .store(crate::cpu_binding::current().unwrap(), Ordering::Relaxed);
        exchange.resumed_local.store(local, Ordering::Relaxed);
        // SAFETY: The second vCPU installed its own scheduler slots before resume.
        unsafe {
            switch(
                exchange.task.load(Ordering::Relaxed),
                exchange.scheduler.load(Ordering::Relaxed),
            )
        };
        std::process::abort()
    }

    fn resume(exchange: &Exchange, context: &mut TaskContext) -> usize {
        let mut scheduler = Registers::default();
        let task = context.registers_ptr();
        exchange.task.store(task, Ordering::Relaxed);
        exchange
            .scheduler
            .store(&raw mut scheduler, Ordering::Relaxed);
        // SAFETY: This thread exclusively owns the suspended context and
        // keeps the scheduler slot alive until the task returns control.
        unsafe { switch(&raw mut scheduler, task) };
        unsafe { libc::pthread_self() }
    }

    #[test]
    fn continuation_resumes_on_the_second_host_thread() {
        let exchange = Arc::new(Exchange::default());
        let mut context =
            TaskContext::new(128 * 1024, task, Arc::as_ptr(&exchange).cast_mut().cast()).unwrap();
        let both_threads_alive = Barrier::new(2);
        let (send, receive) = mpsc::sync_channel(0);
        let (first, second) = thread::scope(|scope| {
            let exchange = exchange.as_ref();
            let barrier = &both_threads_alive;
            let first = scope.spawn(move || {
                crate::cpu_binding::bind(1);
                let id = resume(exchange, &mut context);
                send.send(context).unwrap();
                barrier.wait();
                id
            });
            let second = scope.spawn(move || {
                crate::cpu_binding::bind(2);
                let mut context = receive.recv().unwrap();
                let id = resume(exchange, &mut context);
                barrier.wait();
                id
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_ne!(first, second);
        assert_eq!(exchange.initial_cpu.load(Ordering::Relaxed), 1);
        assert_eq!(exchange.resumed_cpu.load(Ordering::Relaxed), 2);
        assert_eq!(exchange.first_thread.load(Ordering::Relaxed), first);
        assert_eq!(exchange.resumed_thread.load(Ordering::Relaxed), second);
        assert_eq!(exchange.resumed_local.load(Ordering::Relaxed), 0x1234);
    }
}
