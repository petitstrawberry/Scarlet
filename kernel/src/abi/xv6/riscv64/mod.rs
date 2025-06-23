#[macro_use]
mod macros;
mod proc;
mod file;
pub mod fs;
mod pipe;

// pub mod drivers;

use alloc::{boxed::Box, string::ToString, vec::Vec};
use file::{sys_dup, sys_exec, sys_mknod, sys_open, sys_write};
use proc::{sys_exit, sys_fork, sys_wait, sys_getpid};

use crate::{
    abi::{
        xv6::riscv64::{
            file::{sys_close, sys_fstat, sys_link, sys_mkdir, sys_read, sys_unlink}, 
            pipe::sys_pipe, 
            proc::{sys_chdir, sys_sbrk}
        }, 
        AbiModule
    }, arch::{self, Registers}, early_initcall, register_abi, task::elf_loader::load_elf_into_task, vm::{setup_trampoline, setup_user_stack},
    fs::SeekFrom,
};


#[derive(Default, Clone)]
pub struct Xv6Riscv64Abi;

impl AbiModule for Xv6Riscv64Abi {
    fn name() -> &'static str {
        "xv6-riscv64"
    }
    
    fn get_name(&self) -> alloc::string::String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> alloc::boxed::Box<dyn AbiModule> {
        Box::new(self.clone()) // Xv6Riscv64Abi is Copy, so we can dereference and copy
    }
    
    fn handle_syscall(&mut self, trapframe: &mut crate::arch::Trapframe) -> Result<usize, &'static str> {
        syscall_handler(self, trapframe)
    }

    fn can_execute_binary(&self, file_object: &crate::object::KernelObject, file_path: &str) -> Option<u8> {
        // Basic scoring based on file extension and XV6 conventions
        let path_score = if file_path.contains("xv6") || file_path.ends_with(".xv6") {
            40 // Strong XV6 indicator
        } else if file_path.ends_with(".elf") {
            20 // ELF files may be XV6 compatible
        } else {
            10 // Default score for any binary
        };
        
        // Magic byte detection from file content
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                // Check ELF magic bytes (XV6 uses ELF format)
                file_obj.seek(SeekFrom::Start(0)).ok(); // Reset to start
                let mut magic_buffer = [0u8; 4];
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x7F, b'E', b'L', b'F'] {
                            50 // ELF magic bytes match (XV6 is ELF-based)
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
        
        // XV6 should have lower priority than Scarlet native ABI
        if total_score > 20 {
            Some(((total_score / 100) * 80).min(70)) // Scale down to give Scarlet priority
        } else {
            None // Not executable by this ABI
        }
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str], 
        _envp: &[&str],
        task: &mut crate::task::Task,
        trapframe: &mut crate::arch::Trapframe
    ) -> Result<(), &'static str> {
        match file_object.as_file() {
            Some(file_obj) => {
                // Reset task state for XV6 execution
                task.text_size = 0;
                task.data_size = 0;
                task.stack_size = 0;
                
                // Load ELF using XV6-compatible method
                match load_elf_into_task(file_obj, task) {
                    Ok(entry_point) => {
                        // Set the name
                        task.name = argv.get(0).map_or("xv6".to_string(), |s| s.to_string());
                        // Clear page table entries
                        let idx = arch::vm::get_root_pagetable_ptr(task.vm_manager.get_asid()).unwrap();
                        let root_page_table = arch::vm::get_pagetable(idx).unwrap();
                        root_page_table.unmap_all();
                        // Setup the trapframe
                        setup_trampoline(&mut task.vm_manager);
                        // Setup the stack
                        let (_, stack_top) = setup_user_stack(task);
                        let mut stack_pointer = stack_top as usize;

                        let mut arg_ptrs: Vec<u64> = Vec::new();
                        for arg in argv.iter() {
                            let arg_bytes = arg.as_bytes();
                            stack_pointer -= arg_bytes.len() + 1; // +1 for null terminator
                            stack_pointer -= stack_pointer % 16; // Align to 16 bytes

                            unsafe {
                                let translated_stack_pointer = task.vm_manager
                                    .translate_vaddr(stack_pointer)
                                    .unwrap();
                                let stack_slice = core::slice::from_raw_parts_mut(translated_stack_pointer as *mut u8, arg_bytes.len() + 1);
                                stack_slice[..arg_bytes.len()].copy_from_slice(arg_bytes);
                                stack_slice[arg_bytes.len()] = 0; // Null terminator
                            }

                            arg_ptrs.push(stack_pointer as u64); // Store the address of the argument
                        }

                        let argc = arg_ptrs.len();

                        stack_pointer -= argc * 8;
                        stack_pointer -= stack_pointer % 16; // Align to 16 bytes

                        // Push the addresses of the arguments onto the stack
                        unsafe {
                            let translated_stack_pointer = task.vm_manager
                                .translate_vaddr(stack_pointer)
                                .unwrap() as *mut u64;
                            for (i, &arg_ptr) in arg_ptrs.iter().enumerate() {
                                *(translated_stack_pointer.add(i)) = arg_ptr;
                            }
                        }

                        // Set the new entry point for the task
                        task.set_entry_point(entry_point as usize);
                        
                        // Reset task's registers (except for those needed for arguments)
                        task.vcpu.regs = Registers::new();
                        // Set the stack pointer
                        task.vcpu.set_sp(stack_pointer);
                        task.vcpu.regs.reg[11] = stack_pointer as usize; // Set the return value (a0) to 0 in the new proc
                        task.vcpu.regs.reg[10] = argc; // Set argc in a0

                        // Switch to the new task
                        task.vcpu.switch(trapframe);
                        Ok(())
                    },
                    Err(_e) => {
                        Err("Failed to load XV6 ELF binary")
                    }
                }
            },
            None => Err("Invalid file object type for XV6 binary execution"),
        }
    }

    fn get_default_cwd(&self) -> &str {
        "/" // XV6 uses root as default working directory
    }
    
    fn setup_overlay_environment(
        &self,
        target_vfs: &mut crate::fs::VfsManager,
        base_vfs: &alloc::sync::Arc<crate::fs::VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
        // XV6 ABI uses overlay mount with system XV6 tools and config persistence
        let lower_vfs_list = alloc::vec![(base_vfs, system_path)];
        target_vfs.overlay_mount_from(
            Some(base_vfs),             // upper_vfs (base VFS)
            config_path,                // upperdir (read-write persistent layer for XV6)
            lower_vfs_list,             // lowerdir (read-only XV6 system)
            "/"                         // target mount point in task VFS
        ).map_err(|e| {
            crate::println!("Failed to create cross-VFS overlay for XV6 ABI: {}", e.message);
            "Failed to create XV6 overlay environment"
        })
    }
    
    fn setup_shared_resources(
        &self,
        target_vfs: &mut crate::fs::VfsManager,
        base_vfs: &alloc::sync::Arc<crate::fs::VfsManager>,
    ) -> Result<(), &'static str> {
        // XV6 shared resource setup: bind mount common directories and Scarlet gateway
        target_vfs.bind_mount_from(base_vfs, "/home", "/home", false)
            .map_err(|_| "Failed to bind mount /home for XV6")?;
        
        target_vfs.bind_mount_from(base_vfs, "/data/shared", "/data/shared", false)
            .map_err(|_| "Failed to bind mount /data/shared for XV6")?;
        
        // Setup gateway to native Scarlet environment (read-only for security)
        target_vfs.bind_mount_from(base_vfs, "/", "/scarlet", true)
            .map_err(|_| "Failed to bind mount native Scarlet root to /scarlet for XV6")
    }
}

syscall_table! {
    Invalid = 0 => |_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, _trapframe: &mut crate::arch::Trapframe| {
        0
    },
    Fork = 1 => sys_fork,
    Exit = 2 => sys_exit,
    Wait = 3 => sys_wait,
    Pipe = 4 => sys_pipe,
    Read = 5 => sys_read,
    // Kill = 6 => sys_kill,
    Exec = 7 => sys_exec,
    Fstat = 8 => sys_fstat,
    Chdir = 9 => sys_chdir,
    Dup = 10 => sys_dup,
    Getpid = 11 => sys_getpid,
    Sbrk = 12 => sys_sbrk,
    // Sleep = 13 => sys_sleep,
    // Uptime = 14 => sys_uptime,
    Open = 15 => sys_open,
    Write = 16 => sys_write,
    Mknod = 17 => sys_mknod,
    Unlink = 18 => sys_unlink,
    Link = 19 => sys_link,
    Mkdir = 20 => sys_mkdir,
    Close = 21 => sys_close,
}

fn register_xv6_abi() {
    register_abi!(Xv6Riscv64Abi);
}

early_initcall!(register_xv6_abi);