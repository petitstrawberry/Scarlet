use crate::syscall::{Syscall, syscall1};
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
    
    // Allocate closure on heap
    let closure_ptr = Box::into_raw(Box::new(f)) as usize;
    
    // Set up clone flags for thread creation (share VM, FS, Files)
    let mut flags = CloneFlags::new();
    flags.set(CloneFlagsDef::Vm);
    flags.set(CloneFlagsDef::Fs);
    flags.set(CloneFlagsDef::Files);
    
    let result = syscall1(Syscall::Clone, flags.get_raw() as usize);
    
    if result == usize::MAX {
        // Failed to create thread
        unsafe {
            let _ = Box::from_raw(closure_ptr as *mut F);
        }
        return Err("Failed to create thread");
    }
    
    let tid = result as i32;
    
    if tid == 0 {
        // Child thread: execute closure and exit
        unsafe {
            let closure = Box::from_raw(closure_ptr as *mut F);
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
