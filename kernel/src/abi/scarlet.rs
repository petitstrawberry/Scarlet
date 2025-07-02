//! Scarlet Native ABI Module
//! 
//! This module implements the Scarlet ABI for the Scarlet kernel.
//! It provides the necessary functionality for handling system calls
//! and interacting with the Scarlet kernel.
//! 

use alloc::{boxed::Box, string::ToString, sync::Arc};

use crate::{arch::{vm, Registers, Trapframe}, early_initcall, fs::{drivers::overlayfs::OverlayFS, FileSystemError, FileSystemErrorKind, SeekFrom, VfsManager}, register_abi, syscall::syscall_handler, task::elf_loader::load_elf_into_task, vm::{setup_trampoline, setup_user_stack}};

use super::AbiModule;

#[derive(Default, Copy, Clone)]
pub struct ScarletAbi;

impl AbiModule for ScarletAbi {
    fn name() -> &'static str {
        "scarlet"
    }

    fn get_name(&self) -> alloc::string::String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule> {
        Box::new(*self) // ScarletAbi is Copy, so we can dereference and copy
    }

    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        syscall_handler(trapframe)
    }

    fn can_execute_binary(&self, file_object: &crate::object::KernelObject, file_path: &str, current_abi: Option<&dyn crate::abi::AbiModule>) -> Option<u8> {
        // Stage 1: Basic format validation
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                // Check ELF magic bytes (0x7F, 'E', 'L', 'F')
                let mut magic_buffer = [0u8; 4];
                file_obj.seek(SeekFrom::Start(0)).ok(); // Reset to start
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x7F, b'E', b'L', b'F'] {
                            30 // Basic ELF format compatibility
                        } else {
                            return None; // Not an ELF file, cannot execute
                        }
                    }
                    _ => return None // Read failed, cannot determine
                }
            }
            None => return None // Not a file object
        };
        
        let mut confidence = magic_score;
        
        // Stage 2: ELF header checks
        if let Some(file_obj) = file_object.as_file() {
            // Check ELF header for Scarlet-specific OSABI (83)
            let mut osabi_buffer = [0u8; 1];
            file_obj.seek(SeekFrom::Start(7)).ok(); // OSABI is at
            match file_obj.read(&mut osabi_buffer) {
                Ok(bytes_read) if bytes_read == 1 => {
                    if osabi_buffer[0] == 83 { // Scarlet OSABI
                        confidence += 70; // Strong indicator for Scarlet ABI
                    }
                }
                _ => return None // Read failed, cannot determine
            }
        } else {
            return None; // Not a file object
        }
        
        // Stage 3: File path hints
        if file_path.ends_with(".elf") || file_path.contains("scarlet") {
            confidence += 15; // Scarlet-specific path indicators
        }
        
        // Stage 4: ABI inheritance bonus - high priority for same ABI
        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 40; // Strong inheritance bonus for Scarlet Native
            }
        }
        
        Some(confidence.min(100))
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str], 
        _envp: &[&str], // Not implemented yet
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
                        // Set the name from argv[0] or use default
                        task.name = argv.get(0).map_or("Unnamed Task".to_string(), |s| s.to_string());
                        
                        // Clear old page table entries
                        let root_page_table = vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
                        root_page_table.unmap_all();
                        
                        // Setup the new memory environment
                        setup_trampoline(&mut task.vm_manager);
                        let stack_pointer = setup_user_stack(task);

                        // Set the new entry point
                        task.set_entry_point(entry_point as usize);
                        
                        // Reset task's registers for clean start
                        task.vcpu.regs = Registers::new();
                        task.vcpu.set_sp(stack_pointer);

                        // TODO: Setup argv/envp on stack for program arguments
                        // This would involve:
                        // 1. Calculate stack space needed for argv/envp strings
                        // 2. Copy strings to stack memory
                        // 3. Set up argv/envp pointer arrays
                        // 4. Set a0 (argc) and a1 (argv) registers

                        // Switch to the new task
                        task.vcpu.switch(trapframe);
                        Ok(())
                    },
                    Err(e) => {
                        // Log error details
                        crate::println!("ELF loading failed: {}", e.message);
                        Err("Failed to load ELF binary")
                    }
                }
            },
            None => Err("Invalid file object type for binary execution"),
        }
    }

    fn setup_overlay_environment(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
        // crate::println!("Setting up Scarlet overlay environment with system path: {} and config path: {}", system_path, config_path);
        // Scarlet ABI uses overlay mount with system Scarlet tools and config persistence
        let lower_vfs_list = alloc::vec![(base_vfs, system_path)];
        let upper_vfs = base_vfs;
        let fs = match OverlayFS::new_from_paths_and_vfs(Some((upper_vfs, config_path)), lower_vfs_list, "/") {
            Ok(fs) => fs,
            Err(e) => {
                crate::println!("Failed to create overlay filesystem for Scarlet ABI: {}", e.message);
                return Err("Failed to create Scarlet overlay environment");
            }
        };

        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!("Failed to create cross-VFS overlay for Scarlet ABI: {}", e.message);
                Err("Failed to create Scarlet overlay environment")
            }
        }
    }
    
    fn setup_shared_resources(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
    ) -> Result<(), &'static str> {
        // crate::println!("Setting up Scarlet shared resources with base VFS");
        // Scarlet shared resource setup: bind mount common directories and Scarlet gateway
        match create_dir_if_not_exists(target_vfs, "/home") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /home directory for Scarlet: {}", e.message);
                return Err("Failed to create /home directory for Scarlet");
            }
        }

        match target_vfs.bind_mount_from(base_vfs, "/home", "/home") {
            Ok(()) => {}
            Err(e) => {
                // crate::println!("Failed to bind mount /home for Scarlet: {}", e.message);
            }
        }

        match create_dir_if_not_exists(target_vfs, "/data") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /data directory for Scarlet: {}", e.message);
                return Err("Failed to create /data directory for Scarlet");
            }
        }

        match target_vfs.bind_mount_from(base_vfs, "/data/shared", "/data/shared") {
            Ok(()) => {}
            Err(e) => {
                // crate::println!("Failed to bind mount /data/shared for Scarlet: {}", e.message);
            }
        }

        // Setup gateway to native Scarlet environment (read-only for security)
        match create_dir_if_not_exists(target_vfs, "/scarlet") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /scarlet directory for Scarlet: {}", e.message);
                return Err("Failed to create /scarlet directory for Scarlet");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/", "/scarlet") {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!("Failed to bind mount native Scarlet root to /scarlet for Scarlet: {}", e.message);
                return Err("Failed to bind mount native Scarlet root to /scarlet for Scarlet");
            }
        }
    }
}

fn create_dir_if_not_exists(vfs: &Arc<VfsManager>, path: &str) -> Result<(), FileSystemError> {
    match vfs.create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if e.kind == FileSystemErrorKind::AlreadyExists {
                Ok(()) // Directory already exists, nothing to do
            } else {
                Err(e) // Some other error occurred
            }
        }
    }
}

fn register_scarlet_abi() {
    register_abi!(ScarletAbi);
}

early_initcall!(register_scarlet_abi);