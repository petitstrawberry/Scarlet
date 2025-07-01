#[macro_use]
mod macros;
mod proc;
mod file;
pub mod fs;
mod pipe;

// pub mod drivers;

use alloc::{boxed::Box, string::ToString, sync::Arc, vec::Vec};
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
    }, arch::{self, Registers}, early_initcall, fs::{drivers::overlayfs::OverlayFS, SeekFrom, VfsManager}, register_abi, task::elf_loader::load_elf_into_task, vm::{setup_trampoline, setup_user_stack}
};

const MAX_FDS: usize = 1024; // Maximum number of file descriptors

#[derive(Clone)]
pub struct Xv6Riscv64Abi {
    /// File descriptor to handle mapping table (fd -> handle)
    /// None means the fd is not allocated
    fd_to_handle: [Option<u32>; MAX_FDS],
    /// Free file descriptor list for O(1) allocation/deallocation
    free_fds: Vec<usize>,
}

impl Default for Xv6Riscv64Abi {
    fn default() -> Self {
        // Initialize free_fds with all available file descriptors (0 to MAX_FDS-1)
        // Pop from the end so fd 0, 1, 2 are allocated first
        let mut free_fds: Vec<usize> = (0..MAX_FDS).collect();
        free_fds.reverse(); // Reverse so fd 0 is at the end and allocated first
        Self {
            fd_to_handle: [None; MAX_FDS],
            free_fds,
        }
    }
}

impl Xv6Riscv64Abi {
    /// Allocate a new file descriptor and map it to a handle
    pub fn allocate_fd(&mut self, handle: u32) -> Result<usize, &'static str> {
        let fd = if let Some(freed_fd) = self.free_fds.pop() {
            // Reuse a previously freed file descriptor (O(1))
            freed_fd
        } else {
            // No more file descriptors available
            return Err("Too many open files");
        };
        
        self.fd_to_handle[fd] = Some(handle);
        Ok(fd)
    }
    
    /// Get handle from file descriptor
    pub fn get_handle(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            self.fd_to_handle[fd]
        } else {
            None
        }
    }
    
    /// Remove file descriptor mapping
    pub fn remove_fd(&mut self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            if let Some(handle) = self.fd_to_handle[fd].take() {
                // Add the freed fd back to the free list for reuse (O(1))
                self.free_fds.push(fd);
                Some(handle)
            } else {
                None
            }
        } else {
            None
        }
    }
    
    /// Find file descriptor by handle (linear search)
    pub fn find_fd_by_handle(&self, handle: u32) -> Option<usize> {
        for (fd, &mapped_handle) in self.fd_to_handle.iter().enumerate() {
            if let Some(h) = mapped_handle {
                if h == handle {
                    return Some(fd);
                }
            }
        }
        None
    }
    
    /// Remove handle mapping (requires linear search)
    pub fn remove_handle(&mut self, handle: u32) -> Option<usize> {
        if let Some(fd) = self.find_fd_by_handle(handle) {
            self.fd_to_handle[fd] = None;
            self.free_fds.push(fd);
            Some(fd)
        } else {
            None
        }
    }

    /// Initialize standard file descriptors (stdin, stdout, stderr)
    pub fn init_std_fds(&mut self, stdin_handle: u32, stdout_handle: u32, stderr_handle: u32) {
        // XV6 convention: fd 0 = stdin, fd 1 = stdout, fd 2 = stderr
        self.fd_to_handle[0] = Some(stdin_handle);
        self.fd_to_handle[1] = Some(stdout_handle);
        self.fd_to_handle[2] = Some(stderr_handle);
        
        // Remove std fds from free list
        self.free_fds.retain(|&fd| fd != 0 && fd != 1 && fd != 2);
    }
    
    /// Get total number of allocated file descriptors
    pub fn fd_count(&self) -> usize {
        self.fd_to_handle.iter().filter(|&&h| h.is_some()).count()
    }
    
    /// Get the list of allocated file descriptors (for debugging)
    pub fn allocated_fds(&self) -> Vec<usize> {
        self.fd_to_handle.iter()
            .enumerate()
            .filter_map(|(fd, &handle)| if handle.is_some() { Some(fd) } else { None })
            .collect()
    }
}

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

    fn can_execute_binary(
        &self, 
        file_object: &crate::object::KernelObject, 
        file_path: &str,
        current_abi: Option<&dyn AbiModule>
    ) -> Option<u8> {
        // Stage 1: Basic format validation (following implementation guidelines)
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                // Check ELF magic bytes (XV6 uses ELF format)
                let mut magic_buffer = [0u8; 4];
                file_obj.seek(SeekFrom::Start(0)).ok(); // Reset to start
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x7F, b'E', b'L', b'F'] {
                            25 // Basic ELF format compatibility (slightly lower than Scarlet)
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
        
        // Stage 2: Entry point validation (placeholder - could check ELF header)
        // TODO: Add ELF header parsing to validate entry point for XV6 compatibility
        confidence += 10;
        
        // Stage 3: File path hints - XV6 specific patterns
        if file_path.contains("xv6") || file_path.ends_with(".xv6") {
            confidence += 20; // Strong XV6 indicator
        } else if file_path.ends_with(".elf") {
            confidence += 5; // General ELF compatibility
        }
        
        // Stage 4: ABI inheritance bonus - moderate priority for same ABI
        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 15; // Moderate inheritance bonus for XV6
            }
        }
        
        Some(confidence.min(100)) // Standard 0-100 confidence range
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
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
        // XV6 ABI uses overlay mount with system XV6 tools and config persistence
        let lower_vfs_list = alloc::vec![(base_vfs, system_path)];
        let upper_vfs = base_vfs;
        let fs = match OverlayFS::new_from_paths_and_vfs(Some((upper_vfs, config_path)), lower_vfs_list, "/") {
            Ok(fs) => fs,
            Err(e) => {
                crate::println!("Failed to create overlay filesystem for XV6 ABI: {}", e.message);
                return Err("Failed to create XV6 overlay environment");
            }
        }
        ;
        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!("Failed to create cross-VFS overlay for XV6 ABI: {}", e.message);
                Err("Failed to create XV6 overlay environment")
            }
        }
    }
    
    fn setup_shared_resources(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
    ) -> Result<(), &'static str> {
        // XV6 shared resource setup: bind mount common directories and Scarlet gateway
        target_vfs.bind_mount_from(base_vfs, "/home", "/home")
            .map_err(|_| "Failed to bind mount /home for XV6")?;

        target_vfs.bind_mount_from(base_vfs, "/data/shared", "/data/shared")
            .map_err(|_| "Failed to bind mount /data/shared for XV6")?;
        
        // Setup gateway to native Scarlet environment (read-only for security)
        target_vfs.bind_mount_from(base_vfs, "/", "/scarlet")
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