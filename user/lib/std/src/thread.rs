use crate::syscall::{Syscall, syscall1, syscall3};
use crate::task::{CloneFlags, CloneFlagsDef};
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
    use crate::boxed::Box;
    use crate::vec::Vec;

    // Allocate closure on heap
    let closure_ptr = Box::into_raw(Box::new(f)) as usize;

    // Allocate thread stack (2MB)
    const STACK_SIZE: usize = 2 * 1024 * 1024;
    let mut stack = Vec::<u8>::with_capacity(STACK_SIZE);
    unsafe { stack.set_len(STACK_SIZE); }
    let stack_ptr = stack.as_mut_ptr();
    let stack_top = unsafe { stack_ptr.add(STACK_SIZE) } as usize;
    
    // Leak stack (child thread will use it, freed on thread exit)
    core::mem::forget(stack);

    // Set up clone flags for thread creation (share VM, FS, Files)
    let mut flags = CloneFlags::new();
    flags.set(CloneFlagsDef::Vm);
    flags.set(CloneFlagsDef::Fs);
    flags.set(CloneFlagsDef::Files);

    // Clone with stack and closure pointer as argument
    // Child thread will start at same PC with new stack and closure_ptr in a0 register
    let result = syscall3(Syscall::Clone, flags.get_raw() as usize, stack_top, closure_ptr);

    if result == usize::MAX {
        // Failed to create thread
        unsafe {
            let _ = Box::from_raw(closure_ptr as *mut F);
            // TODO: free stack
        }
        return Err("Failed to create thread");
    }

    let tid = result as i32;

    if tid == 0 {
        // Child thread: closure_ptr is in register a0 (first argument)
        // We need to get it via assembly since compiler doesn't know about it
        let closure_arg: usize;
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!(
                "mv {0}, a0",
                out(reg) closure_arg,
            );
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!(
                "mov {0}, x0",
                out(reg) closure_arg,
            );
        }
        
        unsafe {
            let closure = Box::from_raw(closure_arg as *mut F);
            (*closure)();
        }
        crate::task::exit(0);
    } else {
        // Parent: return join handle
        Ok(JoinHandle {
            thread_id: tid as u32,
        })
    }
}
