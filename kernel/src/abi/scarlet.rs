//! Scarlet Native ABI Module
//! 
//! This module implements the Scarlet ABI for the Scarlet kernel.
//! It provides the necessary functionality for handling system calls
//! and interacting with the Scarlet kernel.
//! 

use alloc::string::ToString;

use crate::{arch::{vm, Registers, Trapframe}, early_initcall, register_abi, syscall::syscall_handler, task::elf_loader::load_elf_into_task, vm::{setup_trampoline, setup_user_stack}};

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
        argv: &[&str], 
        envp: &[&str],
        task: &mut crate::task::Task,
        trapframe: &mut Trapframe
    ) -> Result<(), &'static str> {
        // Get file object from KernelObject::File
        match file_object.as_file() {
            Some(file_obj) => {
                task.text_size = 0;
                task.data_size = 0;
                task.stack_size = 0;
                
                // Load the ELF file and replace the current process
                match load_elf_into_task(file_obj, task) {
                    Ok(entry_point) => {
                        // Set the name
                        task.name = argv.get(0).map_or("Unnamed Task".to_string(), |s| s.to_string());
                        // Clear page table entries
                        let root_page_table  = vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
                        root_page_table.unmap_all();
                        // Setup the trapframe
                        setup_trampoline(&mut task.vm_manager);
                        // Setup the stack
                        let stack_pointer = setup_user_stack(task);

                        // Set the new entry point for the task
                        task.set_entry_point(entry_point as usize);
                        
                        // Reset task's registers (except for those needed for arguments)
                        task.vcpu.regs = Registers::new();
                        // Set the stack pointer
                        task.vcpu.set_sp(stack_pointer);

                        // Switch to the new task
                        task.vcpu.switch(trapframe);
                        Ok(())
                    },
                    Err(_e) => {
                        Err("Failed to load ELF binary")
                    }
                }
            },
            None => Err("Invalid file object type for binary execution"),
        }
    }
}

fn register_scarlet_abi() {
    register_abi!(ScarletAbi);
}

early_initcall!(register_scarlet_abi);