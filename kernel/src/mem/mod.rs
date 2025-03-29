pub mod allocator;
pub mod page;

use alloc::{boxed::Box, vec};

use crate::environment::{NUM_OF_CPUS, STACK_SIZE};

#[repr(C, align(4096))]
pub struct Stack {
    pub data: [u8; STACK_SIZE * NUM_OF_CPUS],
}

impl Stack {
    pub fn top(&self) -> usize {
        self.data.as_ptr() as usize
    }
    
    pub fn bottom(&self) -> usize {
        self.data.as_ptr() as usize + self.data.len()
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }
}

#[unsafe(no_mangle)]
pub static mut KERNEL_STACK: Stack = Stack { data: [0; STACK_SIZE * NUM_OF_CPUS] };

pub fn kmalloc(size: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; size].into_boxed_slice()) as *mut u8
}

pub fn kfree(ptr: *mut u8) {
    unsafe {
        let _ = Box::from_raw(ptr);
    }
}