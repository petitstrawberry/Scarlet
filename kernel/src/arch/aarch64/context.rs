//! AArch64 context switching support
//!
//! Provides context switching structures and operations for AArch64.

extern crate alloc;

#[derive(Debug, Clone)]
pub struct KernelContext {
    /// Stack pointer (X30/SP)
    pub sp: u64,
    /// Link register (X30/LR)
    pub lr: u64,
    /// Callee-saved registers X19-X28
    pub x: [u64; 10],
    /// Kernel stack for this context (None = uninitialized)
    /// Using Box<[u8]> to directly allocate on heap without stack overflow
    pub kernel_stack: Option<alloc::boxed::Box<[u8]>>,
}

impl KernelContext {
    pub fn new() -> Self {
        // Directly allocate on heap to avoid stack overflow
        let kernel_stack = alloc::vec![0u8; crate::environment::TASK_KERNEL_STACK_SIZE].into_boxed_slice();
        
        KernelContext {
            sp: 0,
            lr: 0,
            x: [0; 10],
            kernel_stack: Some(kernel_stack),
        }
    }
    
    pub const fn new_uninit() -> Self {
        KernelContext {
            sp: 0,
            lr: 0,
            x: [0; 10],
            kernel_stack: None,
        }
    }
    
    /// Get the bottom of the kernel stack
    pub fn get_kernel_stack_bottom(&self) -> u64 {
        match &self.kernel_stack {
            Some(stack) => stack.as_ptr() as u64 + stack.len() as u64,
            None => 0,
        }
    }
    
    pub fn get_kernel_stack_memory_area(&self) -> Option<crate::vm::vmem::MemoryArea> {
        match &self.kernel_stack {
            Some(stack) => Some(crate::vm::vmem::MemoryArea::new(stack.as_ptr() as usize, self.get_kernel_stack_bottom() as usize - 1)),
            None => None,
        }
    }

    pub fn get_kernel_stack_ptr(&self) -> Option<*const u8> {
        match &self.kernel_stack {
            Some(stack) => Some(stack.as_ptr()),
            None => None,
        }
    }

    pub fn set_kernel_stack(&mut self, stack: alloc::boxed::Box<[u8]>) {
        self.kernel_stack = Some(stack);
        self.sp = self.get_kernel_stack_bottom();
    }

    /// Set entry point for this context
    pub fn set_entry_point(&mut self, entry_point: u64) {
        self.lr = entry_point;
    }

    pub fn get_entry_point(&self) -> u64 {
        self.lr
    }
}