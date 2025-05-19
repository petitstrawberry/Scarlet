//! Scarlet Native ABI Module
//! 
//! This module implements the Scarlet ABI for the Scarlet kernel.
//! It provides the necessary functionality for handling system calls
//! and interacting with the Scarlet kernel.
//! 

use crate::{arch::Trapframe, early_initcall, register_abi, syscall::syscall_handler};

use super::AbiModule;

#[derive(Default, Copy, Clone)]
pub struct ScarletAbi;

impl AbiModule for ScarletAbi {
    fn name() -> &'static str {
        "scarlet"
    }

    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        syscall_handler(trapframe)
    }
}

fn register_scarlet_abi() {
    register_abi!(ScarletAbi);
}

early_initcall!(register_scarlet_abi);