//! Scarlet Native ABI Module (x86_64)
//!
//! This module implements the Scarlet ABI for x86_64 architecture.

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};

use crate::{
    arch::Trapframe,
    early_initcall,
    fs::VfsManager,
    register_abi,
    syscall::syscall_handler,
    task::elf_loader::{LoadTarget, analyze_and_load_elf_with_strategy},
};

use crate::abi::AbiModule;

/// Scarlet ABI for x86_64
#[derive(Debug, Clone, Default)]
pub struct ScarletAbi {
    /// TLS pointer
    tls_pointer: Option<usize>,
}

impl ScarletAbi {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AbiModule for ScarletAbi {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "scarlet"
    }

    fn get_name(&self) -> String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }

    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        syscall_handler(trapframe)
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut Trapframe,
    ) -> Result<(), &'static str> {
        use crate::task::elf_loader::LoadStrategy;

        let entry_point =
            analyze_and_load_elf_with_strategy(file_object, task, LoadStrategy::Native)?;

        let sp = crate::vm::setup_user_stack(task, argv, envp)?;

        trapframe.rip = entry_point as u64;
        trapframe.rsp = sp as u64;
        trapframe.rflags = 0x202; // IF flag set

        Ok(())
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        let mut confidence = 0u8;

        if let Some(file) = file_object.as_file() {
            let mut buf = [0u8; 4];
            use crate::object::capability::StreamOps;
            let _ = file.read_at(0, &mut buf);

            if &buf == b"\x7fELF" {
                confidence += 30;
            }

            if file.read_at(4, &mut buf[..1]).is_ok() && buf[0] == 2 {
                confidence += 15;
            }

            let mut e_machine = [0u8; 2];
            if file.read_at(18, &mut e_machine).is_ok() {
                let machine = u16::from_le_bytes(e_machine);
                if machine == 0x3D {
                    confidence += 30;
                }
            }
        }

        if file_path.contains("scarlet") {
            confidence = confidence.saturating_add(15);
        }

        if let Some(abi) = current_abi {
            if abi.get_name() == Self::name() {
                confidence = confidence.saturating_add(25);
            }
        }

        Some(confidence.min(100))
    }

    fn choose_load_address(&self, elf_type: u16, target: LoadTarget) -> Option<u64> {
        use crate::task::elf_loader::LoadTarget;
        const ET_DYN: u16 = 3;

        if elf_type == ET_DYN {
            match target {
                LoadTarget::Executable => Some(0x0000_0040_0000_0000),
                LoadTarget::Interpreter => Some(0x0000_0070_0000_0000),
                LoadTarget::SharedLibrary => None,
            }
        } else {
            None
        }
    }

    fn set_tls_pointer(&mut self, ptr: usize) {
        self.tls_pointer = Some(ptr);
    }

    fn get_tls_pointer(&self) -> Option<usize> {
        self.tls_pointer
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

fn abi_scarlet_x86_64_init() {
    register_abi!(ScarletAbi);
}

early_initcall!(abi_scarlet_x86_64_init);
