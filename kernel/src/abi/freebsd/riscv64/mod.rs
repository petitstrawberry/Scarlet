#[macro_use]
mod macros;
mod errno;
mod fs;
mod mm;
mod pipe;
mod proc;

use alloc::{boxed::Box, string::ToString, sync::Arc, vec, vec::Vec};
use crate::{
    abi::AbiModule,
    arch::{IntRegisters, Trapframe},
    early_initcall,
    fs::{SeekFrom, VfsManager},
    register_abi,
    task::elf_loader::{
        LoadStrategy, LoadTarget, analyze_and_load_elf_with_strategy,
    },
    vm::setup_user_stack,
};

const MAX_FDS: usize = 1024; // Maximum number of file descriptors

/// FreeBSD RISC-V64 ABI implementation
#[derive(Clone)]
pub struct FreeBsdRiscv64Abi {
    /// Task namespace for FreeBSD PID management
    namespace: Arc<crate::task::namespace::TaskNamespace>,
    /// File descriptor to handle mapping table (fd -> handle)
    /// None means the fd is not allocated
    /// Vec to avoid stack overflow during initialization
    fd_to_handle: Vec<Option<u32>>,
    /// File descriptor flags (e.g., FD_CLOEXEC)
    /// Vec to avoid stack overflow during initialization
    fd_flags: Vec<u32>,
    /// Free file descriptor list for O(1) allocation/deallocation
    free_fds: Vec<usize>,
}

impl Default for FreeBsdRiscv64Abi {
    fn default() -> Self {
        // Initialize free_fds with all available file descriptors (0 to MAX_FDS-1)
        // Pop from the end so fd 0, 1, 2 are allocated first
        let mut free_fds: Vec<usize> = (0..MAX_FDS).collect();
        free_fds.reverse(); // Reverse so fd 0 is at the end and allocated first

        // Use root namespace by default for cross-ABI task visibility
        let namespace = crate::task::namespace::get_root_namespace().clone();

        Self {
            namespace,
            fd_to_handle: vec![None; MAX_FDS],
            fd_flags: vec![0; MAX_FDS],
            free_fds,
        }
    }
}

impl FreeBsdRiscv64Abi {
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

    /// Remove file descriptor mapping and clear its flags
    pub fn remove_fd(&mut self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            if let Some(handle) = self.fd_to_handle[fd].take() {
                self.fd_flags[fd] = 0; // Clear flags when removing fd
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

    /// Initialize standard file descriptors (stdin, stdout, stderr)
    pub fn init_std_fds(&mut self, stdin_handle: u32, stdout_handle: u32, stderr_handle: u32) {
        // FreeBSD convention: fd 0 = stdin, fd 1 = stdout, fd 2 = stderr
        self.fd_to_handle[0] = Some(stdin_handle);
        self.fd_to_handle[1] = Some(stdout_handle);
        self.fd_to_handle[2] = Some(stderr_handle);

        // Remove std fds from free list
        self.free_fds.retain(|&fd| fd != 0 && fd != 1 && fd != 2);
    }
}

impl AbiModule for FreeBsdRiscv64Abi {
    fn name() -> &'static str {
        "freebsd-riscv64"
    }

    fn get_name(&self) -> alloc::string::String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> alloc::boxed::Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }

    fn handle_syscall(
        &mut self,
        trapframe: &mut crate::arch::Trapframe,
    ) -> Result<usize, &'static str> {
        syscall_handler(self, trapframe)
    }

    fn get_task_namespace(&self) -> Arc<crate::task::namespace::TaskNamespace> {
        self.namespace.clone()
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        // ELF constants
        const EM_RISCV: u16 = 243; // RISC-V machine type
        const ELFOSABI_FREEBSD: u8 = 9; // FreeBSD OS/ABI identifier
        
        // Check if this is an ELF file
        let file = file_object.as_file()?;
        
        let mut elf_header = [0u8; 64];
        file.seek(SeekFrom::Start(0)).ok();
        if file.read(&mut elf_header).ok()? < 64 {
            return None;
        }
        
        // Check ELF magic number
        if &elf_header[0..4] != b"\x7fELF" {
            return None;
        }
        
        // Check if it's 64-bit (ELFCLASS64)
        if elf_header[4] != 2 {
            return None;
        }
        
        // Check if it's RISC-V
        let e_machine = u16::from_le_bytes([elf_header[18], elf_header[19]]);
        if e_machine != EM_RISCV {
            return None;
        }
        
        // Check OS/ABI field (e_ident[EI_OSABI])
        let osabi = elf_header[7];
        
        let mut confidence = 0u8;
        
        // Basic format check: valid ELF + RISC-V 64-bit
        confidence += 30;
        
        // Check for FreeBSD OS/ABI marker
        if osabi == ELFOSABI_FREEBSD {
            confidence += 40; // Strong indicator for FreeBSD
        }
        
        // File path hints
        if file_path.contains("freebsd") {
            confidence += 15;
        }
        
        // ABI inheritance bonus
        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 15;
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
        trapframe: &mut Trapframe,
    ) -> Result<(), &'static str> {
        // Execute FreeBSD ELF binary using the ELF loader
        match file_object.as_file() {
            Some(file_obj) => {
                // Use the generic ELF loader with FreeBSD-specific configuration
                let load_strategy = LoadStrategy {
                    choose_base_address: |target, needs_relocation| {
                        match (target, needs_relocation) {
                            (LoadTarget::MainProgram, false) => 0, // Static executables
                            (LoadTarget::MainProgram, true) => 0x10000, // PIE executables
                            (LoadTarget::Interpreter, _) => 0x40000000, // Dynamic linker
                            (LoadTarget::SharedLib, _) => 0x50000000, // Shared libraries
                        }
                    },
                    resolve_interpreter: |requested| requested.map(|s| s.to_string()),
                };

                match analyze_and_load_elf_with_strategy(file_obj, task, &load_strategy) {
                    Ok(load_result) => {
                        // Setup stack with arguments and environment variables
                        let (_, stack_top) = setup_user_stack(task);
                        let mut sp = stack_top as usize;

                        // --- 1. Push argument and environment strings ---
                        let mut arg_vaddrs: Vec<u64> = Vec::new();
                        for &arg in argv.iter() {
                            let len = arg.len() + 1;
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

                        // --- 2. Align stack to 16 bytes ---
                        sp = sp & !0xF;

                        // Build auxiliary vector based on the ELF loading result
                        use crate::task::elf_loader::build_auxiliary_vector;
                        let auxv = build_auxiliary_vector(&load_result);

                        // --- Calculate total size needed for structured data ---
                        let auxv_size = auxv.len() * 16; // Each auxv entry is 16 bytes
                        let envp_size = (env_vaddrs.len() + 1) * 8; // +1 for NULL terminator
                        let argv_size = (arg_vaddrs.len() + 1) * 8; // +1 for NULL terminator
                        let argc_size = 8;
                        let total_structured_size = auxv_size + envp_size + argv_size + argc_size;

                        // Align the total size and calculate final sp
                        let aligned_size = (total_structured_size + 15) & !15;
                        sp -= aligned_size;
                        let final_sp = sp;
                        let mut current_pos = final_sp;

                        // --- 1. Argument count (argc) ---
                        let argc = argv.len() as u64;
                        unsafe {
                            *(task.vm_manager.translate_vaddr(current_pos).unwrap() as *mut u64) =
                                argc;
                        }
                        current_pos += 8;

                        // --- 2. Argument pointer array (argv) ---
                        for &arg_vaddr in arg_vaddrs.iter() {
                            unsafe {
                                *(task.vm_manager.translate_vaddr(current_pos).unwrap()
                                    as *mut u64) = arg_vaddr;
                            }
                            current_pos += 8;
                        }
                        // NULL terminator for argv
                        unsafe {
                            *(task.vm_manager.translate_vaddr(current_pos).unwrap() as *mut u64) =
                                0;
                        }
                        current_pos += 8;

                        // --- 3. Environment pointer array (envp) ---
                        for &env_vaddr in env_vaddrs.iter() {
                            unsafe {
                                *(task.vm_manager.translate_vaddr(current_pos).unwrap()
                                    as *mut u64) = env_vaddr;
                            }
                            current_pos += 8;
                        }
                        // NULL terminator for envp
                        unsafe {
                            *(task.vm_manager.translate_vaddr(current_pos).unwrap() as *mut u64) =
                                0;
                        }
                        current_pos += 8;

                        // --- 4. Auxiliary vector (auxv) ---
                        for auxv_entry in auxv.iter() {
                            unsafe {
                                let paddr = task.vm_manager.translate_vaddr(current_pos).unwrap()
                                    as *mut u64;
                                *paddr = auxv_entry.a_type;
                                *(paddr.add(1)) = auxv_entry.a_val;
                            }
                            current_pos += 16; // Each entry is 16 bytes
                        }

                        // Use the aligned final_sp
                        sp = final_sp;

                        task.set_entry_point(load_result.entry_point as usize);
                        task.vcpu.iregs = IntRegisters::new(); // Clear registers
                        task.vcpu.set_sp(sp); // Set stack pointer

                        // Initialize trapframe with clean state
                        trapframe.regs = task.vcpu.iregs;
                        trapframe.epc = load_result.entry_point;

                        // Switch to the new task
                        task.vcpu.switch(trapframe);
                        Ok(())
                    }
                    Err(e) => {
                        crate::println!("Failed to load FreeBSD ELF binary: {:?}", e);
                        Err("Failed to load FreeBSD ELF binary")
                    }
                }
            }
            None => Err("Invalid file object type for FreeBSD binary execution"),
        }
    }

    fn get_default_cwd(&self) -> &str {
        "/" // FreeBSD uses root as default working directory
    }

    fn initialize_from_existing_handles(
        &mut self,
        _task: &mut crate::task::Task,
    ) -> Result<(), &'static str> {
        self.init_std_fds(
            0, // stdin handle
            1, // stdout handle
            2, // stderr handle
        );
        Ok(())
    }
}

syscall_table! {
    Invalid = 0 => |_abi: &mut crate::abi::freebsd::riscv64::FreeBsdRiscv64Abi, _trapframe: &mut crate::arch::Trapframe| {
        0
    },
    Exit = 1 => proc::sys_exit,
    Read = 3 => fs::sys_read,
    Write = 4 => fs::sys_write,
    Open = 5 => fs::sys_open,
    Close = 6 => fs::sys_close,
    Brk = 17 => proc::sys_brk,
    GetPid = 20 => proc::sys_getpid,
    GetUid = 24 => proc::sys_getuid,
    GetEuid = 25 => proc::sys_geteuid,
    GetPpid = 39 => proc::sys_getppid,
    GetEgid = 43 => proc::sys_getegid,
    GetGid = 47 => proc::sys_getgid,
    IoCtl = 54 => fs::sys_ioctl,
    Lseek = 62 => fs::sys_lseek,
    Munmap = 73 => mm::sys_munmap,
    Mprotect = 74 => mm::sys_mprotect,
    Fcntl = 92 => fs::sys_fcntl,
    Mmap = 197 => mm::sys_mmap,
    Pipe = 42 => pipe::sys_pipe,
}

fn register_freebsd_abi() {
    register_abi!(FreeBsdRiscv64Abi);
}

early_initcall!(register_freebsd_abi);
