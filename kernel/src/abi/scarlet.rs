use crate::{arch::Trapframe, early_initcall, register_abi, syscall::syscall_handler};

use super::AbiModule;

#[derive(Default)]
pub struct ScarletAbi;

impl ScarletAbi {
    pub fn new() -> Self {
        ScarletAbi {}
    }
}

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