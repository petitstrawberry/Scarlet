//! Scarlet Native ABI Module
//! 
//! This module implements the Scarlet ABI for the Scarlet kernel.
//! It provides the necessary functionality for handling system calls
//! and interacting with the Scarlet kernel.
//! 

use alloc::{boxed::Box, collections::btree_map::BTreeMap, format, string::{String, ToString}, sync::Arc, vec::Vec};

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
                task.brk = None;

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
                        let stack_pointer = setup_user_stack(task).1;

                        // Set the new entry point
                        task.set_entry_point(entry_point as usize);
                        
                        // Reset task's registers for clean start
                        task.vcpu.regs = Registers::new();
                        task.vcpu.set_sp(stack_pointer);

                        // Setup argv/envp on stack following Unix and RISC-V conventions
                        let (adjusted_sp, argv_ptr) = self.setup_arguments_on_stack(task, argv, envp, stack_pointer)?;
                        task.vcpu.set_sp(adjusted_sp);
                        
                        // Set RISC-V calling convention registers
                        // a0 (reg[10]) = argc
                        // a1 (reg[11]) = argv pointer
                        task.vcpu.regs.reg[10] = argv.len(); // argc
                        task.vcpu.regs.reg[11] = argv_ptr; // argv array pointer

                        // crate::println!("Executing binary: {} with entry point: {:#x}", task.name, entry_point);
                        // crate::println!("Arguments: {:?}", argv);
                        // crate::println!("Environment: {:?}", envp);
                        // crate::println!("argv pointer set to: {:#x}", argv_ptr);
                        // crate::println!("Environment pointer set to: {:#x}", env_ptr);

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

    fn normalize_env_to_scarlet(&self, envp: &mut Vec<String>) {
        // Scarlet ABI is already in canonical format, but ensure all paths are absolute
        // Modify in-place to avoid allocations
        
        for env_var in envp.iter_mut() {
            if let Some(eq_pos) = env_var.find('=') {
                let key = &env_var[..eq_pos];
                let value = &env_var[eq_pos + 1..];
                
                let normalized_value = match key {
                    "PATH" | "LD_LIBRARY_PATH" => {
                        // Ensure all paths are in absolute Scarlet namespace format
                        self.normalize_path_to_absolute_scarlet(value)
                    }
                    "HOME" => {
                        // Ensure home directory is absolute
                        if value.starts_with('/') {
                            value.to_string()
                        } else {
                            format!("/home/{}", value)
                        }
                    }
                    _ => value.to_string(), // Most variables pass through unchanged
                };
                
                // Update in-place if value changed
                let new_env_var = format!("{}={}", key, normalized_value);
                if new_env_var != *env_var {
                    *env_var = new_env_var;
                }
            }
        }
    }
    
    fn denormalize_env_from_scarlet(&self, envp: &mut Vec<String>) {
        // For Scarlet ABI, canonical format is the native format
        // But ensure proper Scarlet-specific defaults exist
        
        // Convert to temporary map for easier processing
        let mut env_map = BTreeMap::new();
        for env_var in envp.iter() {
            if let Some(eq_pos) = env_var.find('=') {
                let key = env_var[..eq_pos].to_string();
                let value = env_var[eq_pos + 1..].to_string();
                env_map.insert(key, value);
            }
        }
        
        // Add defaults if they don't exist
        if !env_map.contains_key("PATH") {
            env_map.insert("PATH".to_string(), "/system/scarlet/bin:/bin:/usr/bin".to_string());
        }
        
        if !env_map.contains_key("SHELL") {
            env_map.insert("SHELL".to_string(), "/system/scarlet/bin/sh".to_string());
        }
        
        // Convert back to Vec<String> format
        envp.clear();
        for (key, value) in env_map.iter() {
            envp.push(format!("{}={}", key, value));
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
            Err(_e) => {
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
            Err(_e) => {
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

impl ScarletAbi {
    /// Setup argc, argv, and envp on the user stack following Unix conventions
    /// 
    /// Standard Unix stack layout (from high to low addresses):
    /// ```
    /// [high addresses]
    /// envp strings (null-terminated)
    /// argv strings (null-terminated)  
    /// envp[] array (null-terminated pointer array)
    /// argv[] array (null-terminated pointer array)
    /// argc (integer)
    /// [low addresses - returned stack pointer]
    /// ```
    /// 
    /// # Arguments
    /// * `task` - The task to set up arguments for
    /// * `argv` - Command line arguments
    /// * `envp` - Environment variables
    /// * `initial_sp` - Initial stack pointer from setup_user_stack
    /// 
    /// # Returns
    /// Tuple of (new stack pointer, argv array pointer)
    fn setup_arguments_on_stack(
        &self,
        task: &mut crate::task::Task,
        argv: &[&str],
        envp: &[&str],
        initial_sp: usize
    ) -> Result<(usize, usize), &'static str> {
        // Calculate total size needed
        let argc = argv.len();
        let envc = envp.len();
        
        // Calculate string sizes (including null terminators)
        let argv_strings_size: usize = argv.iter().map(|s| s.len() + 1).sum();
        let envp_strings_size: usize = envp.iter().map(|s| s.len() + 1).sum();
        
        // Calculate pointer array sizes (including null terminators)
        let argv_array_size = (argc + 1) * core::mem::size_of::<usize>(); // +1 for NULL terminator
        let envp_array_size = (envc + 1) * core::mem::size_of::<usize>(); // +1 for NULL terminator
        let argc_size = core::mem::size_of::<usize>();
        
        // Total space needed
        let total_size = argc_size + argv_array_size + envp_array_size + argv_strings_size + envp_strings_size;
        
        // Align to 16-byte boundary for RISC-V ABI compliance
        let aligned_total_size = (total_size + 15) & !15;
        
        // Calculate new stack pointer
        let new_sp = initial_sp - aligned_total_size;
        
        // Layout from new_sp (low) to initial_sp (high):
        // argc | argv[] | envp[] | argv_strings | envp_strings
        
        let mut current_addr = new_sp;
        
        // 1. Write argc
        self.write_to_stack_memory(task, current_addr, &argc.to_le_bytes())?;
        current_addr += argc_size;
        
        // 2. Save argv array pointer for return value
        let argv_ptr = current_addr;
        
        // 3. Calculate string positions first
        let argv_strings_start = current_addr + argv_array_size + envp_array_size;
        let envp_strings_start = argv_strings_start + argv_strings_size;
        
        // 4. Write argv[] array
        let mut string_addr = argv_strings_start;
        for i in 0..argc {
            self.write_to_stack_memory(task, current_addr, &string_addr.to_le_bytes())?;
            current_addr += core::mem::size_of::<usize>();
            string_addr += argv[i].len() + 1; // Move to next string position
        }
        // NULL terminate argv[]
        let null_ptr: usize = 0;
        self.write_to_stack_memory(task, current_addr, &null_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();
        
        // 5. Write envp[] array
        string_addr = envp_strings_start;
        for i in 0..envc {
            self.write_to_stack_memory(task, current_addr, &string_addr.to_le_bytes())?;
            current_addr += core::mem::size_of::<usize>();
            string_addr += envp[i].len() + 1; // Move to next string position
        }
        // NULL terminate envp[]
        self.write_to_stack_memory(task, current_addr, &null_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();
        
        // 6. Write argv strings
        for arg in argv {
            self.write_string_to_stack(task, current_addr, arg)?;
            current_addr += arg.len() + 1; // +1 for null terminator
        }
        
        // 7. Write envp strings
        for env in envp {
            self.write_string_to_stack(task, current_addr, env)?;
            current_addr += env.len() + 1; // +1 for null terminator
        }
        
        Ok((new_sp, argv_ptr))
    }
    
    /// Write bytes to stack memory using virtual memory translation
    fn write_to_stack_memory(
        &self,
        task: &mut crate::task::Task,
        vaddr: usize,
        data: &[u8]
    ) -> Result<(), &'static str> {
        match task.vm_manager.translate_vaddr(vaddr) {
            Some(paddr) => {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        paddr as *mut u8,
                        data.len()
                    );
                }
                Ok(())
            }
            None => Err("Failed to translate virtual address for stack write")
        }
    }
    
    /// Write a null-terminated string to stack memory
    fn write_string_to_stack(
        &self,
        task: &mut crate::task::Task,
        vaddr: usize,
        string: &str
    ) -> Result<(), &'static str> {
        // Write the string content
        self.write_to_stack_memory(task, vaddr, string.as_bytes())?;
        // Write null terminator
        self.write_to_stack_memory(task, vaddr + string.len(), &[0u8])?;
        Ok(())
    }

    /// Normalize path string to absolute Scarlet namespace format
    /// 
    /// This ensures all paths in PATH-like variables are absolute and
    /// in the proper Scarlet namespace format.
    fn normalize_path_to_absolute_scarlet(&self, path_value: &str) -> String {
        let paths: Vec<&str> = path_value.split(':').collect();
        let mut normalized_paths = Vec::new();
        
        for path in paths {
            if path.starts_with('/') {
                // Already absolute - ensure it's in proper Scarlet namespace
                if path.starts_with("/system/scarlet/") || path.starts_with("/scarlet/") {
                    normalized_paths.push(path.to_string());
                } else {
                    // Map standard paths to Scarlet namespace
                    let mapped_path = match path {
                        "/bin" => "/system/scarlet/bin",
                        "/usr/bin" => "/system/scarlet/usr/bin",
                        "/usr/local/bin" => "/system/scarlet/usr/local/bin",
                        "/sbin" => "/system/scarlet/sbin",
                        "/usr/sbin" => "/system/scarlet/usr/sbin",
                        "/lib" => "/system/scarlet/lib",
                        "/usr/lib" => "/system/scarlet/usr/lib",
                        "/usr/local/lib" => "/system/scarlet/usr/local/lib",
                        _ => path, // Keep other absolute paths as-is
                    };
                    normalized_paths.push(mapped_path.to_string());
                }
            } else if !path.is_empty() {
                // Relative paths - prefix with current working directory or make absolute
                normalized_paths.push(format!("/{}", path));
            }
            // Skip empty paths
        }
        
        normalized_paths.join(":")
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