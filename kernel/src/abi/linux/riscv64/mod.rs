#[macro_use]
mod macros;
mod proc;
mod mm;
mod fs;
// mod file;
// pub mod fs;
// mod pipe;

// pub mod drivers;

use alloc::{boxed::Box, string::ToString, sync::Arc, vec::Vec};
// use file::{sys_dup, sys_exec, sys_mknod, sys_open, sys_write};
// use proc::{sys_exit, sys_fork, sys_wait, sys_getpid};

use crate::{
    abi::AbiModule, arch::{self, Registers}, early_initcall, environment::PAGE_SIZE, fs::{drivers::overlayfs::OverlayFS, FileSystemError, FileSystemErrorKind, SeekFrom, VfsManager}, register_abi, task::elf_loader::load_elf_into_task, vm::{setup_trampoline, setup_user_stack}
};

const MAX_FDS: usize = 1024; // Maximum number of file descriptors

#[derive(Clone)]
pub struct LinuxRiscv64Abi {
    /// File descriptor to handle mapping table (fd -> handle)
    /// None means the fd is not allocated
    fd_to_handle: [Option<u32>; MAX_FDS],
    /// Free file descriptor list for O(1) allocation/deallocation
    free_fds: Vec<usize>,
}

impl Default for LinuxRiscv64Abi {
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

impl LinuxRiscv64Abi {
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
        // Linux convention: fd 0 = stdin, fd 1 = stdout, fd 2 = stderr
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

impl AbiModule for LinuxRiscv64Abi {
    fn name() -> &'static str {
        "linux-riscv64"
    }
    
    fn get_name(&self) -> alloc::string::String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> alloc::boxed::Box<dyn AbiModule> {
        Box::new(self.clone()) // LinuxRiscv64Abi is Copy, so we can dereference and copy
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
                // Check ELF magic bytes (Linux uses ELF format)
                let mut magic_buffer = [0u8; 4];
                file_obj.seek(SeekFrom::Start(0)).ok(); // Reset to start
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x7F, b'E', b'L', b'F'] {
                            35 // Basic ELF format compatibility (slightly lower than Scarlet)
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
            // Check ELF header for System-V ABI (Linux uses System-V ABI)
            let mut osabi_buffer = [0u8; 1];
            file_obj.seek(SeekFrom::Start(7)).ok(); // OSABI is at
            match file_obj.read(&mut osabi_buffer) {
                Ok(bytes_read) if bytes_read == 1 => {
                    if osabi_buffer[0] == 0 { // System-V ABI
                        confidence += 50; // Strong indicator for System-V ABI
                    }
                }
                _ => return None // Read failed, cannot determine
            }
        } else {
            return None; // Not a file object
        }

        // Stage 3: File path hints - Linux specific patterns
        if file_path.contains("linux") || file_path.ends_with(".linux") {
            confidence += 20; // Strong Linux indicator
        } else if file_path.ends_with(".elf") {
            confidence += 5; // General ELF compatibility
        }
        
        // Stage 4: ABI inheritance bonus - moderate priority for same ABI
        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 15; // Moderate inheritance bonus for Linux
            }
        }
        
        Some(confidence.min(100)) // Standard 0-100 confidence range
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &mut crate::task::Task,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<(), &'static str> {
        match file_object.as_file() {
            Some(file_obj) => {
                // Reset task state for Linux execution
                task.text_size = 0;
                task.data_size = 0;
                task.stack_size = 0;
                task.brk = None;

                // Load ELF using Linux-compatible method
                match load_elf_into_task(file_obj, task) {
                    Ok(entry_point) => {
                        // Set the name
                        task.name = argv.get(0).map_or("linux".to_string(), |s| s.to_string());
                        // Clear page table entries
                        let idx = arch::vm::get_root_pagetable_ptr(task.vm_manager.get_asid()).unwrap();
                        let root_page_table = arch::vm::get_pagetable(idx).unwrap();
                        root_page_table.unmap_all();
                        // Setup the trampoline
                        setup_trampoline(&mut task.vm_manager);
                        // Setup the stack
                        let (_, stack_top) = setup_user_stack(task);
                        let mut sp = stack_top as usize;

                        // --- 1. Argument strings ---
                        let mut arg_vaddrs: Vec<u64> = Vec::new();
                        for &arg in argv.iter() {
                            let len = arg.len() + 1; // +1 for null terminator
                            sp -= len;

                            let vaddr = sp;
                            unsafe {
                                let paddr = task.vm_manager.translate_vaddr(vaddr).unwrap();
                                let slice = core::slice::from_raw_parts_mut(paddr as *mut u8, len);
                                slice[..len - 1].copy_from_slice(arg.as_bytes());
                                slice[len - 1] = 0; // Null terminator
                            }
                            arg_vaddrs.push(vaddr as u64);
                        }

                        let mut env_vaddrs: Vec<u64> = Vec::new();
                        for &env in envp.iter() {
                            let len = env.len() + 1;
                            sp -= len;
                            let vaddr = sp;
                            unsafe {
                                let paddr = task.vm_manager.translate_vaddr(vaddr).unwrap();
                                let slice = core::slice::from_raw_parts_mut(paddr as *mut u8, len);
                                slice[..len - 1].copy_from_slice(env.as_bytes());
                                slice[len - 1] = 0; // Null terminator
                            }
                            env_vaddrs.push(vaddr as u64);
                        }

                        // --- 2. Stack alignment ---
                        sp &= !0xF;

                        // --- 3. Auxiliary vector (auxv) ---
                        // Set up the auxiliary vector on the stack.
                        // The C runtime (such as musl) reads important information like page size from here.
                        const AT_NULL: u64 = 0;     // End of vector
                        const AT_PAGESZ: u64 = 6;   // Key indicating page size

                        // Push AT_NULL entry (type=0, value=0)
                        sp -= 16; // auxv entry is 16 bytes (u64 type, u64 val)
                        unsafe {
                            let paddr = task.vm_manager.translate_vaddr(sp).unwrap() as *mut u64;
                            *paddr = AT_NULL;
                            *(paddr.add(1)) = 0;
                        }

                        // Push AT_PAGESZ entry (type=6, value=PAGE_SIZE)
                        sp -= 16;
                        unsafe {
                            let paddr = task.vm_manager.translate_vaddr(sp).unwrap() as *mut u64;
                            *paddr = AT_PAGESZ;
                            *(paddr.add(1)) = PAGE_SIZE as u64;
                        }

                        // --- 4. envp pointer array ---
                        sp -= 8; // NULL terminator for envp
                        unsafe {
                            *(task.vm_manager.translate_vaddr(sp).unwrap() as *mut u64) = 0;
                        }
                        for &env_vaddr in env_vaddrs.iter().rev() {
                            sp -= 8;
                            unsafe {
                                *(task.vm_manager.translate_vaddr(sp).unwrap() as *mut u64) = env_vaddr;
                            }
                        }

                        // --- 5. argv pointer array ---
                        sp -= 8; // NULL terminator for argv
                        unsafe {
                            *(task.vm_manager.translate_vaddr(sp).unwrap() as *mut u64) = 0;
                        }
                        for &arg_vaddr in arg_vaddrs.iter().rev() {
                            sp -= 8;
                            unsafe {
                                *(task.vm_manager.translate_vaddr(sp).unwrap() as *mut u64) = arg_vaddr;
                            }
                        }

                        // --- 6. argc ---
                        let argc = argv.len() as u64;
                        sp -= 8;
                        unsafe {
                            *(task.vm_manager.translate_vaddr(sp).unwrap() as *mut u64) = argc;
                        }

                        task.set_entry_point(entry_point as usize);
                        task.vcpu.regs = Registers::new(); // Clear registers
                        task.vcpu.set_sp(sp); // Set stack pointer

                        // Switch to the new task
                        task.vcpu.switch(trapframe);
                        Ok(())
                    }
                    Err(e) => {
                        crate::println!("Failed to load Linux ELF binary: {:?}", e);
                        Err("Failed to load Linux ELF binary")
                    }
                }
            }
            None => Err("Invalid file object type for Linux binary execution"),
        }
    }

    fn get_default_cwd(&self) -> &str {
        "/" // Linux uses root as default working directory
    }
    
    fn setup_overlay_environment(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
        // crate::println!("Setting up Linux overlay environment with system path: {} and config path: {}", system_path, config_path);
        // Linux ABI uses overlay mount with system Linux tools and config persistence
        let lower_vfs_list = alloc::vec![(base_vfs, system_path)];
        let upper_vfs = base_vfs;
        let fs = match OverlayFS::new_from_paths_and_vfs(Some((upper_vfs, config_path)), lower_vfs_list, "/") {
            Ok(fs) => fs,
            Err(e) => {
                crate::println!("Failed to create overlay filesystem for Linux ABI: {}", e.message);
                return Err("Failed to create Linux overlay environment");
            }
        };
        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!("Failed to create cross-VFS overlay for Linux ABI: {}", e.message);
                Err("Failed to create Linux overlay environment")
            }
        }
    }
    
    fn setup_shared_resources(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
    ) -> Result<(), &'static str> {
        // crate::println!("Setting up Linux shared resources with base VFS");
        // Linux shared resource setup: bind mount common directories and Scarlet gateway
        match create_dir_if_not_exists(target_vfs, "/home") {
            Ok(()) => {}
            Err(e) => {
                // crate::println!("Failed to create /home directory for Linux: {}", e.message);
                return Err("Failed to create /home directory for Linux");
            }
        }

        match target_vfs.bind_mount_from(base_vfs, "/home", "/home") {
            Ok(()) => {}
            Err(e) => {
                // crate::println!("Failed to bind mount /home for Linux: {}", e.message);
            }
        }

        match create_dir_if_not_exists(target_vfs, "/data") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /data directory for Linux: {}", e.message);
                return Err("Failed to create /data directory for Linux");
            }
        }

        match target_vfs.bind_mount_from(base_vfs, "/data/shared", "/data/shared") {
            Ok(()) => {}
            Err(e) => {
                // crate::println!("Failed to bind mount /data/shared for Linux: {}", e.message);
            }
        }

        // Setup gateway to native Scarlet environment (read-only for security)
        match create_dir_if_not_exists(target_vfs, "/scarlet") {
            Ok(()) => {}
            Err(e) => {
                crate::println!("Failed to create /scarlet directory for Linux: {}", e.message);
                return Err("Failed to create /scarlet directory for Linux");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/", "/scarlet") {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!("Failed to bind mount native Scarlet root to /scarlet for Linux: {}", e.message);
                return Err("Failed to bind mount native Scarlet root to /scarlet for Linux");
            }
        }
    }

    fn initialize_from_existing_handles(&mut self, task: &mut crate::task::Task) -> Result<(), &'static str> {
        // task.handle_table.close_all();
        self.init_std_fds(
            0, // stdin handle
            1, // stdout handle
            2, // stderr handle
        );
        Ok(())
    }
}

syscall_table! {
    Invalid = 0 => |_abi: &mut crate::abi::linux::riscv64::LinuxRiscv64Abi, _trapframe: &mut crate::arch::Trapframe| {
        0
    },
    Ioctl = 29 => fs::sys_ioctl,
    Write = 64 => fs::sys_write,
    Writev = 66 => fs::sys_writev,
    NewFstAtAt = 79 => fs::sys_newfstatat,
    SetTidAddress = 96 => proc::sys_set_tid_address,
    Exit = 93 => proc::sys_exit,
    ExitGroup = 94 => proc::sys_exit_group,
    SetRobustList = 99 => proc::sys_set_robust_list,
    Uname = 160 => proc::sys_uname,
    GetUid = 174 => proc::sys_getuid,
    Brk = 214 => proc::sys_brk,
    Munmap = 215 => mm::sys_munmap,
    Mmap = 222 => mm::sys_mmap,
    Mprotect = 226 => mm::sys_mprotect,
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

fn register_linux_abi() {
    register_abi!(LinuxRiscv64Abi);
}

early_initcall!(register_linux_abi);