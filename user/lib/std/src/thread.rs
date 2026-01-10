use crate::boxed::Box;
use crate::syscall::{Syscall, syscall1, syscall4};
use crate::task::{CloneFlags, CloneFlagsDef};
use crate::vec::Vec;
use core::time::Duration;

/// Thread sleep
pub fn sleep(dur: Duration) -> i32 {
    let nanosecs = dur.as_nanos() as usize;
    syscall1(Syscall::Sleep, nanosecs) as i32
}

/// Thread builder (simplified)
pub struct Builder {
    name: Option<&'static str>,
}

impl Builder {
    pub fn new() -> Self {
        Builder { name: None }
    }

    pub fn name(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    pub fn spawn<F>(self, f: F) -> Result<JoinHandle, &'static str>
    where
        F: FnOnce() + Send + 'static,
    {
        spawn_impl(f, self.name)
    }
}

/// Spawn a new thread
pub fn spawn<F>(f: F) -> JoinHandle
where
    F: FnOnce() + Send + 'static,
{
    spawn_impl(f, None).expect("Failed to spawn thread")
}

/// Join handle for waiting on thread completion
pub struct JoinHandle {
    thread_id: u32,
}

impl JoinHandle {
    /// Wait for the thread to finish
    pub fn join(self) -> Result<(), &'static str> {
        // Wait for child thread to exit
        let (pid, _status) = crate::task::waitpid(self.thread_id as i32, 0);
        if pid < 0 {
            Err("Failed to join thread")
        } else {
            Ok(())
        }
    }
}

// Internal implementation
fn spawn_impl<F>(f: F, _name: Option<&'static str>) -> Result<JoinHandle, &'static str>
where
    F: FnOnce() + Send + 'static,
{
    // Keep the allocation type consistent with the typed trampoline.
    // Using a trait object here and reconstructing it as `Box<F>` is UB.
    let closure: Box<F> = Box::new(f);
    let closure_ptr = Box::into_raw(closure) as usize;

    // Allocate thread stack (256KB - reasonable size for embedded system).
    // Keep this as `u8` so we don't require high-alignment allocations from the allocator.
    const STACK_SIZE: usize = 256 * 1024;
    const STACK_ALIGN: usize = 16;
    let stack: Vec<u8> = crate::vec![0u8; STACK_SIZE];
    let stack_base = stack.as_ptr() as usize;
    let stack_end = stack_base + STACK_SIZE;

    // Stack grows downward, so set SP to top of stack minus 16 bytes.
    // Keep it 16-byte aligned (AArch64/RISC-V ABI friendly).
    let stack_top = (stack_end - 16) & !(STACK_ALIGN - 1);

    // Leak stack (child thread will use it, freed on thread exit)
    core::mem::forget(stack);

    // Set up clone flags for thread creation (share VM, FS, Files)
    let mut flags = CloneFlags::new();
    flags.set(CloneFlagsDef::Vm);
    flags.set(CloneFlagsDef::Fs);
    flags.set(CloneFlagsDef::Files);

    // Use a typed trampoline that knows about F
    // Clone with: flags, stack, trampoline function, closure pointer
    let result = syscall4(
        Syscall::Clone,
        flags.get_raw() as usize,
        stack_top,
        thread_typed_trampoline::<F> as *const () as usize,
        closure_ptr,
    );

    if result == usize::MAX {
        // Failed to create thread
        unsafe {
            let _ = Box::from_raw(closure_ptr as *mut F);
            // TODO: free stack
        }
        return Err("Failed to create thread");
    }

    // Parent: return join handle
    Ok(JoinHandle {
        thread_id: result as u32,
    })
}

/// Typed thread trampoline - knows the closure type F
#[inline(never)]
extern "C" fn thread_typed_trampoline<F>(closure_ptr: usize) -> !
where
    F: FnOnce() + Send + 'static,
{
    // Reconstruct the Box<F> from the pointer
    unsafe {
        let closure = Box::from_raw(closure_ptr as *mut F);
        closure();
    }

    // Exit the thread
    crate::task::exit(0);
}
