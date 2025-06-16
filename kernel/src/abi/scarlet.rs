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

    fn can_execute_binary(&self, file_object: &crate::object::KernelObject, file_path: &str) -> Option<u8> {
        // Basic scoring based on file extension
        let path_score = if file_path.ends_with(".elf") || file_path.contains("scarlet") {
            30 // Basic score from file extension
        } else {
            0
        };
        
        // Magic byte detection from file content
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                // Check ELF magic bytes (0x7F, 'E', 'L', 'F')
                let mut magic_buffer = [0u8; 4];
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x7F, b'E', b'L', b'F'] {
                            60 // ELF magic bytes match
                        } else {
                            0
                        }
                    }
                    _ => 0 // Read failed or insufficient size
                }
            }
            None => 0 // Not a file object
        };
        
        let total_score = path_score + magic_score;
        
        // Check minimum score threshold
        if total_score > 0 {
            Some(total_score.min(100)) // Limit to 0-100 range
        } else {
            None // Not executable by this ABI
        }
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        _argv: &[&str], 
        _envp: &[&str],
        _task: &mut crate::task::Task,
        _trapframe: &mut Trapframe
    ) -> Result<(), &'static str> {
        // Get file object from KernelObject::File
        match file_object.as_file() {
            Some(_file_obj) => {
                // TODO: Use ELF loader to load and execute binary
                // This part will use the task::elf_loader module to
                // parse and load ELF files
                
                // Complete implementation should:
                // 1. Load ELF into task memory space
                // 2. Set task entry point and stack
                // 3. Update trapframe registers (PC, SP)
                // 4. Set trapframe.set_return_value(0) for success (optional)
                // 5. Either return Ok(()) to let syscall chain handle return value,
                //    or perform direct context switch here
                
                // Implementation example:
                // use crate::task::elf_loader;
                // let elf_data = elf_loader::load_elf_from_file(file_obj)?;
                // elf_loader::map_elf_to_task(elf_data, task)?;
                // trapframe.set_pc(elf_data.entry_point);
                // trapframe.set_sp(new_stack_pointer);
                // trapframe.set_return_value(0); // Optional: set success return value
                // // sys_execve() will use trapframe.get_return_value() as its return
                // task.vcpu.switch(trapframe); // Optional: direct switch
                
                Err("ELF loading not yet implemented")
            },
            None => Err("Invalid file object type for binary execution"),
        }
    }
}

fn register_scarlet_abi() {
    register_abi!(ScarletAbi);
}

early_initcall!(register_scarlet_abi);