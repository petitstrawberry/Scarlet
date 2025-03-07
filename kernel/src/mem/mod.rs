pub mod allocator;

use crate::environment::{NUM_OF_CPUS, STACK_SIZE};

#[repr(C, align(16))]
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