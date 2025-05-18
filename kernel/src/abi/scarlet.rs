use crate::{arch::Trapframe, syscall::syscall_handler};

use super::AbiModule;

pub struct ScarletAbi {
    pub name: &'static str,
}

impl ScarletAbi {
    pub fn new() -> Self {
        ScarletAbi {
            name: "scarlet",
        }
    }
}

impl AbiModule for ScarletAbi {
    fn name(&self) -> &'static str {
        self.name
    }

    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        syscall_handler(trapframe)
    }
}