#[macro_use]
mod macros;
mod proc;

pub use super::generic::errno;
// pub mod drivers;

use alloc::{
    boxed::Box, collections::BTreeMap, format, string::ToString, sync::Arc, vec, vec::Vec,
};

use super::generic::time::PosixTimer;
use crate::{
    abi::{
        AbiModule,
        linux::generic::{self, fs, futex, mm, pipe, signal, socket, time},
    },
    arch::{self, IntRegisters, Trapframe},
    early_initcall,
    fs::{
        FileSystemError, FileSystemErrorKind, SeekFrom, VfsManager, drivers::overlayfs::OverlayFS,
    },
    register_abi,
    task::elf_loader::{
        ExecutionMode, LoadStrategy, LoadTarget, analyze_and_load_elf_with_strategy,
    },
    vm::setup_user_stack,
};

const MAX_FDS: usize = 1024; // Maximum number of file descriptors

#[derive(Clone, Default)]
pub struct LinuxThreadState {
    pub parent_tid_ptr: Option<usize>,
    pub child_tid_ptr: Option<usize>,
    pub clear_child_tid_ptr: Option<usize>,
    pub robust_list_head: Option<usize>,
    pub robust_list_len: usize,
    pub tls_pointer: Option<usize>,
    /// Linux Thread Group ID (TGID). For processes, TGID == PID of group leader.
    /// 0 means uninitialized and should fall back to Task ID.
    pub tgid: usize,
    /// Internal: set by sys_clone prior to cloning to indicate CLONE_THREAD semantics
    pub pending_clone_is_thread: bool,
}

#[derive(Clone)]
pub struct LinuxAarch64Abi {
    /// File descriptor to handle mapping table (fd -> handle)
    /// None means the fd is not allocated
    /// Vec to avoid stack overflow during initialization
    fd_to_handle: Vec<Option<u32>>,
    /// File descriptor flags (e.g., FD_CLOEXEC)
    /// Vec to avoid stack overflow during initialization
    fd_flags: Vec<u32>,
    /// File status flags (e.g., O_NONBLOCK) for F_GETFL/F_SETFL
    /// Vec to avoid stack overflow during initialization
    file_status_flags: Vec<u32>,
    /// Free file descriptor list for O(1) allocation/deallocation
    free_fds: Vec<usize>,
    /// Signal handling state
    pub signal_state: Arc<spin::Mutex<signal::SignalState>>,
    /// Linux-specific per-task thread state (isolated inside ABI)
    thread_state: LinuxThreadState,
    /// POSIX timers created via timer_create
    posix_timers: BTreeMap<u64, PosixTimer>,
    /// Next timer identifier (Linux timer_t)
    next_timer_id: u64,
}

impl Default for LinuxAarch64Abi {
    fn default() -> Self {
        // Initialize free_fds with all available file descriptors (0 to MAX_FDS-1)
        // Pop from the end so fd 0, 1, 2 are allocated first
        let mut free_fds: Vec<usize> = (0..MAX_FDS).collect();
        free_fds.reverse(); // Reverse so fd 0 is at the end and allocated first
        Self {
            fd_to_handle: vec![None; MAX_FDS],
            fd_flags: vec![0; MAX_FDS],
            file_status_flags: vec![0; MAX_FDS],
            free_fds,
            signal_state: Arc::new(spin::Mutex::new(signal::SignalState::new())),
            thread_state: LinuxThreadState::default(),
            posix_timers: BTreeMap::new(),
            next_timer_id: 1,
        }
    }
}

impl LinuxAarch64Abi {
    pub fn thread_state(&self) -> &LinuxThreadState {
        &self.thread_state
    }
    pub fn thread_state_mut(&mut self) -> &mut LinuxThreadState {
        &mut self.thread_state
    }
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

    /// Allocate a specific file descriptor and map it to a handle
    pub fn allocate_specific_fd(&mut self, fd: usize, handle: u32) -> Result<(), &'static str> {
        if fd >= MAX_FDS {
            return Err("File descriptor out of range");
        }

        // Check if the fd is already in use
        if self.fd_to_handle[fd].is_some() {
            return Err("File descriptor already in use");
        }

        // Remove from free list if present
        if let Some(pos) = self.free_fds.iter().position(|&x| x == fd) {
            self.free_fds.remove(pos);
        }

        self.fd_to_handle[fd] = Some(handle);
        Ok(())
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
                self.file_status_flags[fd] = 0; // Clear status flags as well
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

    /// Initialize standard file descriptors (stdin, stdout, stderr)
    pub fn init_std_fds(&mut self, stdin_handle: u32, stdout_handle: u32, stderr_handle: u32) {
        // Linux convention: fd 0 = stdin, fd 1 = stdout, fd 2 = stderr
        self.fd_to_handle[0] = Some(stdin_handle);
        self.fd_to_handle[1] = Some(stdout_handle);
        self.fd_to_handle[2] = Some(stderr_handle);

        // Remove std fds from free list
        self.free_fds.retain(|&fd| fd != 0 && fd != 1 && fd != 2);
    }

    /// Get file descriptor flags
    pub fn get_fd_flags(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            Some(self.fd_flags[fd])
        } else {
            None
        }
    }

    /// Set file descriptor flags
    pub fn set_fd_flags(&mut self, fd: usize, flags: u32) -> Result<(), &'static str> {
        use crate::abi::linux::generic::fs::FD_CLOEXEC;
        use crate::{object::handle::SpecialSemantics, task::mytask};

        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            let handle = self.fd_to_handle[fd].unwrap();
            self.fd_flags[fd] = flags;

            // Update handle metadata to sync FD_CLOEXEC with SpecialSemantics::CloseOnExec
            if let Some(task) = mytask() {
                if let Some(current_metadata) = task.handle_table.get_metadata(handle) {
                    let mut new_metadata = current_metadata.clone();

                    if flags & FD_CLOEXEC != 0 {
                        // Set CloseOnExec if FD_CLOEXEC flag is present
                        new_metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
                    } else {
                        // Remove CloseOnExec if FD_CLOEXEC flag is not present
                        if matches!(
                            new_metadata.special_semantics,
                            Some(SpecialSemantics::CloseOnExec)
                        ) {
                            new_metadata.special_semantics = None;
                        }
                    }

                    // Update the metadata
                    let _ = task.handle_table.update_metadata(handle, new_metadata);
                }
            }

            Ok(())
        } else {
            Err("Invalid file descriptor")
        }
    }

    /// Get file status flags (F_GETFL)
    pub fn get_file_status_flags(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            Some(self.file_status_flags[fd])
        } else {
            None
        }
    }

    /// Set file status flags (F_SETFL)
    pub fn set_file_status_flags(&mut self, fd: usize, flags: u32) -> Result<(), &'static str> {
        if fd < MAX_FDS && self.fd_to_handle[fd].is_some() {
            self.file_status_flags[fd] = flags;
            Ok(())
        } else {
            Err("Invalid file descriptor")
        }
    }

    /// Get total number of allocated file descriptors
    pub fn fd_count(&self) -> usize {
        self.fd_to_handle.iter().filter(|&&h| h.is_some()).count()
    }

    /// Get the list of allocated file descriptors (for debugging)
    pub fn allocated_fds(&self) -> Vec<usize> {
        self.fd_to_handle
            .iter()
            .enumerate()
            .filter_map(|(fd, &handle)| if handle.is_some() { Some(fd) } else { None })
            .collect()
    }

    /// Process pending signals and handle them according to Linux semantics
    /// Returns true if execution should be interrupted (signal handler called or process terminated)
    pub fn process_signals(&self, trapframe: &mut Trapframe) -> bool {
        let mut signal_state = self.signal_state.lock();
        signal::process_pending_signals_with_state(&mut *signal_state, trapframe)
    }

    // No pthread/TLS probing helpers; user space owns pthread layout.

    /// Handle incoming event from Scarlet event system and convert to signal if applicable
    pub fn handle_event_direct(&self, event: &crate::ipc::event::Event) {
        if let Some(signal) = signal::handle_event_to_signal(event) {
            let mut signal_state = self.signal_state.lock();
            signal_state.add_pending(signal);
        }
    }

    /// Check if there are pending signals ready for delivery
    pub fn has_pending_signals(&self) -> bool {
        let signal_state = self.signal_state.lock();
        signal_state.next_deliverable_signal().is_some()
    }

    /// Allocate a unique timer identifier for POSIX timers
    pub fn allocate_posix_timer_id(&mut self) -> u64 {
        let mut id = self.next_timer_id;
        if id == 0 {
            id = 1;
        }
        self.next_timer_id = id.wrapping_add(1);
        if self.next_timer_id == 0 {
            self.next_timer_id = 1;
        }
        id
    }

    /// Store a POSIX timer definition tracked by this ABI instance
    pub fn store_posix_timer(&mut self, timer: PosixTimer) {
        self.posix_timers.insert(timer.id, timer);
    }

    /// Retrieve a reference to a stored POSIX timer
    pub fn get_posix_timer(&self, id: u64) -> Option<&PosixTimer> {
        self.posix_timers.get(&id)
    }

    /// Remove a POSIX timer from this ABI instance
    pub fn remove_posix_timer(&mut self, id: u64) -> Option<PosixTimer> {
        self.posix_timers.remove(&id)
    }
}

impl AbiModule for LinuxAarch64Abi {
    fn name() -> &'static str {
        "linux-aarch64"
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

    fn handle_event(
        &self,
        event: crate::ipc::Event,
        target_task_id: u32,
    ) -> Result<(), &'static str> {
        // Convert event to signal if applicable
        if let Some(signal) = signal::handle_event_to_signal(&event) {
            let scheduler = crate::sched::scheduler::get_scheduler();
            let target_task = scheduler
                .get_task_by_id(target_task_id as usize)
                .ok_or("Target task not found")?;

            // Check if this is a fatal signal that should terminate immediately
            match signal {
                signal::LinuxSignal::SIGKILL
                | signal::LinuxSignal::SIGTERM
                | signal::LinuxSignal::SIGINT => {
                    // Fatal signals: terminate task immediately
                    let exit_code = 128 + (signal as i32); // Standard Unix exit code for signals
                    crate::early_println!(
                        "Linux ABI: Terminating task {} due to signal {} (exit code {})",
                        target_task.get_id(),
                        signal as u32,
                        exit_code
                    );
                    target_task.exit(exit_code);
                }
                _ => {
                    // Other signals: add to pending (for future handler implementation)
                    let mut signal_state = self.signal_state.lock();
                    signal_state.add_pending(signal);
                    crate::early_println!(
                        "Linux ABI: Added signal {} to pending for task {}",
                        signal as u32,
                        target_task_id
                    );
                }
            }
        }

        // For non-signal events, just acknowledge
        Ok(())
    }

    fn on_task_cloned(
        &mut self,
        _parent_task: &crate::task::Task,
        _child_task: &mut crate::task::Task,
        _flags: crate::task::CloneFlags,
    ) -> Result<(), &'static str> {
        // Child ABI state is a clone of parent's state (including pointers set in sys_clone).
        // Preserve child-specific fields (child_tid_ptr, clear_child_tid_ptr, tls_pointer),
        // and only update TGID and transient flags.
        let mut ts = self.thread_state.clone();
        let parent_tgid = ts.tgid;
        let is_thread = ts.pending_clone_is_thread;

        // Initialize child's TGID based on whether this was a thread (CLONE_THREAD) or a process clone.
        ts.tgid = if is_thread {
            if parent_tgid != 0 {
                parent_tgid
            } else {
                _parent_task.get_id()
            }
        } else {
            _child_task.get_id()
        };

        // Clear transient flag in the child copy
        ts.pending_clone_is_thread = false;

        // No debug dump or futex watch arming.

        // Commit updated thread state
        self.thread_state = ts;
        Ok(())
    }

    fn on_task_exit(&mut self, task: &mut crate::task::Task) {
        // No pthread/TLS structure probing at exit; user space owns pthread list.
        // Linux semantics: if clear_child_tid is set, write 0 and FUTEX_WAKE.
        if let Some(ptr) = self.thread_state.clear_child_tid_ptr {
            if let Some(paddr) = task.vm_manager.translate_vaddr(ptr) {
                unsafe {
                    *(paddr as *mut i32) = 0;
                }
                // Wake one waiter on the TID address as per Linux semantics
                let _ = super::generic::futex::wake_address(ptr, 1);
            }
        }
        // TODO: robust list-based wakeups for owned mutexes at thread exit.
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
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
                    _ => return None, // Read failed, cannot determine
                }
            }
            None => return None, // Not a file object
        };

        let mut confidence = magic_score;

        // Stage 2: ELF header checks
        if let Some(file_obj) = file_object.as_file() {
            // Check ELF header for System-V ABI (Linux uses System-V ABI)
            let mut osabi_buffer = [0u8; 1];
            file_obj.seek(SeekFrom::Start(7)).ok(); // OSABI is at
            match file_obj.read(&mut osabi_buffer) {
                Ok(bytes_read) if bytes_read == 1 => {
                    if osabi_buffer[0] == 0 {
                        // System-V ABI
                        confidence += 50; // Strong indicator for System-V ABI
                    }
                }
                _ => return None, // Read failed, cannot determine
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

                // Load ELF using Linux-compatible method with dynamic linking support
                match analyze_and_load_elf_with_strategy(
                    file_obj,
                    task,
                    &LoadStrategy {
                        choose_base_address: |target, needs_relocation| {
                            match (target, needs_relocation) {
                                (LoadTarget::MainProgram, false) => 0, // Static executables
                                (LoadTarget::MainProgram, true) => 0x40000000, // PIE executables
                                (LoadTarget::Interpreter, _) => 0x40000000, // Dynamic linker
                                (LoadTarget::SharedLib, _) => 0x50000000, // Shared libraries
                            }
                        },
                        resolve_interpreter: |requested| {
                            // Map interpreter paths to system paths
                            requested.map(|path| {
                                if path.starts_with("/lib/ld-") || path.starts_with("/lib64/ld-") {
                                    // Map to our system path
                                    format!("/scarlet/system/linux-aarch64{}", path)
                                } else {
                                    path.to_string()
                                }
                            })
                        },
                    },
                ) {
                    Ok(load_result) => {
                        // Set the name
                        task.name = argv.get(0).map_or("linux".to_string(), |s| s.to_string());
                        // Do not resolve pthread/TLS or arm futex-based watches in kernel.
                        crate::println!("Program segments:");
                        task.vm_manager.with_memmaps(|mm| {
                            for map in mm.values() {
                                crate::println!("  VA: {:#x}-{:#x} -> PA: {:#x}-{:#x} (perm: {:#x}, shared: {})",
                                    map.vmarea.start, map.vmarea.end,
                                    map.pmarea.start, map.pmarea.end,
                                    map.permissions, map.is_shared);
                            }
                        });
                        crate::println!("=================================");

                        // Clear page table entries
                        let idx =
                            arch::vm::get_root_pagetable_ptr(task.vm_manager.get_asid()).unwrap();
                        let root_page_table = arch::vm::get_pagetable(idx).unwrap();
                        root_page_table.unmap_all();
                        // Setup the trampoline
                        arch::vm::setup_trampoline_for_user(&mut task.vm_manager);
                        // Setup the stack following Linux ABI standard layout
                        let (_, stack_top) = setup_user_stack(task);
                        let mut sp = stack_top as usize;

                        // For dynamic executables, reserve space for the dynamic linker's stack frame
                        if let ExecutionMode::Dynamic { .. } = &load_result.mode {
                            // Reserve 96 bytes for the dynamic linker's stack frame
                            // This matches what _dlstart_c expects
                            sp -= 96;
                            // Zero out the reserved space
                            unsafe {
                                let paddr = task.vm_manager.translate_vaddr(sp).unwrap();
                                let slice = core::slice::from_raw_parts_mut(paddr as *mut u8, 96);
                                slice.fill(0);
                            }
                        }

                        // --- 1. Argument and environment strings (at high addresses) ---
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
                            // crate::println!("Setting up env: {}", env);
                            // Debug: Print raw bytes for LD_LIBRARY_PATH
                            // if env.starts_with("LD_LIBRARY_PATH=") {
                            //     crate::println!("LD_LIBRARY_PATH env string length: {}", env.len());
                            //     crate::println!("LD_LIBRARY_PATH raw bytes: {:?}", env.as_bytes());
                            //     for (i, &byte) in env.as_bytes().iter().enumerate() {
                            //         if byte < 32 || byte > 126 {
                            //             crate::println!("  Non-printable byte at {}: 0x{:02x} ('{}' is printable)",
                            //                           i, byte, (byte >= 32 && byte <= 126));
                            //         }
                            //     }
                            // }
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

                        // --- 2. Platform-specific padding and auxiliary vector ---
                        // Align to 16 bytes before starting structured data

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
                        let aligned_size = (total_structured_size + 15) & !15; // Round up to 16-byte boundary
                        sp -= aligned_size;
                        let final_sp = sp;
                        let mut current_pos = final_sp;

                        // --- Place data from the calculated position ---

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
                        // crate::println!("Setting up auxiliary vector with {} entries:", auxv.len());
                        for auxv_entry in auxv.iter() {
                            // crate::println!("  auxv[{}]: type={:#x} value={:#x} @ sp={:#x}",
                            //     i, auxv_entry.a_type, auxv_entry.a_val, current_pos);
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

                        // // Debug: Dump stack contents around the final SP
                        // crate::println!("DEBUG: Final stack dump from sp={:#x}:", sp);
                        // for i in 0..32 {
                        //     let addr = sp + (i * 8);
                        //     if let Some(paddr) = task.vm_manager.translate_vaddr(addr) {
                        //         let value = unsafe { *(paddr as *const u64) };
                        //         crate::println!("  [{:#x}] = {:#018x} ({})", addr, value,
                        //             core::str::from_utf8(&value.to_le_bytes()).unwrap_or("<invalid>"));
                        //     }
                        // }

                        task.set_entry_point(load_result.entry_point as usize);
                        task.vcpu.iregs = IntRegisters::new(); // Clear registers
                        task.vcpu.set_sp(sp); // Set stack pointer

                        // Initialize trapframe with clean state
                        trapframe.regs = task.vcpu.iregs;
                        trapframe.set_current_pc(load_result.entry_point);
                        // crate::println!("DEBUG: Set trapframe PC to {:#x}", load_result.entry_point);

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
        let fs = match OverlayFS::new_from_paths_and_vfs(
            Some((upper_vfs, config_path)),
            lower_vfs_list,
            "/",
        ) {
            Ok(fs) => fs,
            Err(e) => {
                crate::println!(
                    "Failed to create overlay filesystem for Linux ABI: {}",
                    e.message
                );
                return Err("Failed to create Linux overlay environment");
            }
        };
        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!(
                    "Failed to create cross-VFS overlay for Linux ABI: {}",
                    e.message
                );
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
            Err(_e) => {
                // crate::println!("Failed to create /home directory for Linux: {}", _e.message);
                return Err("Failed to create /home directory for Linux");
            }
        }

        match target_vfs.bind_mount_from(base_vfs, "/home", "/home") {
            Ok(()) => {}
            Err(_e) => {
                // crate::println!("Failed to bind mount /home for Linux: {}", _e.message);
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
            Err(_e) => {
                // crate::println!("Failed to bind mount /data/shared for Linux: {}", _e.message);
            }
        }

        // Setup devices directory
        match create_dir_if_not_exists(target_vfs, "/dev") {
            Ok(()) => {}
            Err(_e) => {
                crate::println!("Failed to create /dev directory for Linux: {}", _e.message);
                return Err("Failed to create /dev directory for Linux");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/dev", "/dev") {
            Ok(()) => {}
            Err(_e) => {
                crate::println!("Failed to bind mount /dev for Linux: {}", _e.message);
                return Err("Failed to bind mount /dev for Linux");
            }
        }

        // Setup gateway to native Scarlet environment (read-only for security)
        match create_dir_if_not_exists(target_vfs, "/scarlet") {
            Ok(()) => {}
            Err(_e) => {
                crate::println!(
                    "Failed to create /scarlet directory for Linux: {}",
                    _e.message
                );
                return Err("Failed to create /scarlet directory for Linux");
            }
        }
        match target_vfs.bind_mount_from(base_vfs, "/", "/scarlet") {
            Ok(()) => Ok(()),
            Err(_e) => {
                crate::println!(
                    "Failed to bind mount native Scarlet root to /scarlet for Linux: {}",
                    _e.message
                );
                return Err("Failed to bind mount native Scarlet root to /scarlet for Linux");
            }
        }
    }

    fn initialize_from_existing_handles(
        &mut self,
        _task: &mut crate::task::Task,
    ) -> Result<(), &'static str> {
        // _task.handle_table.close_all();
        self.init_std_fds(
            0, // stdin handle
            1, // stdout handle
            2, // stderr handle
        );
        // Initialize TGID for the task at ABI attach time
        self.thread_state.tgid = _task.get_id();
        Ok(())
    }
}

syscall_table! {
    Invalid = 0 => |_abi: &mut crate::abi::linux::LinuxAbi, _trapframe: &mut crate::arch::Trapframe| {
        0
    },
    Getcwd = 17 => fs::sys_getcwd,
    EpollCreate1 = 20 => fs::sys_epoll_create1,
    EpollCtl = 21 => fs::sys_epoll_ctl,
    EpollPwait = 22 => fs::sys_epoll_pwait,
    Dup = 23 => fs::sys_dup,
    Dup3 = 24 => fs::sys_dup3,
    Fcntl = 25 => fs::sys_fcntl,
    Ioctl = 29 => fs::sys_ioctl,
    MkdirAt = 34 => fs::sys_mkdirat,
    UnlinkAt = 35 => fs::sys_unlinkat,
    LinkAt = 37 => fs::sys_linkat,
    FaccessAt = 48 => fs::sys_faccessat,
    Chdir = 49 => fs::sys_chdir,
    Fchmod = 52 => fs::sys_fchmod,
    OpenAt = 56 => fs::sys_openat,
    Close = 57 => fs::sys_close,
    Pipe2 = 59 => pipe::sys_pipe2,
    GetDents64 = 61 => fs::sys_getdents64,
    Lseek = 62 => fs::sys_lseek,
    Read = 63 => fs::sys_read,
    Write = 64 => fs::sys_write,
    Readv = 65 => fs::sys_readv,
    Writev = 66 => fs::sys_writev,
    Pread64 = 67 => fs::sys_pread64,
    Pwrite64 = 68 => fs::sys_pwrite64,
    Pselect6 = 72 => fs::sys_pselect6,
    Ppoll = 73 => fs::sys_ppoll,
    NewFstAtAt = 79 => fs::sys_newfstatat,
    NewFstat = 80 => fs::sys_newfstat,
    ReadLinkAt = 78 => fs::sys_readlinkat,
    Fsync = 82 => fs::sys_fsync,
    Exit = 93 => generic::proc::sys_exit,
    ExitGroup = 94 => generic::proc::sys_exit_group,
    SetTidAddress = 96 => generic::proc::sys_set_tid_address,
    Futex = 98 => futex::sys_futex,
    SetRobustList = 99 => generic::proc::sys_set_robust_list,
    Nanosleep = 101 => time::sys_nanosleep,
    TimerCreate = 107 => time::sys_timer_create,
    TimerGettime = 108 => time::sys_timer_gettime,
    TimerGetoverrun = 109 => time::sys_timer_getoverrun,
    TimerSettime = 110 => time::sys_timer_settime,
    TimerDelete = 111 => time::sys_timer_delete,
    ClockGettime = 113 => time::sys_clock_gettime,
    ClockGetres = 114 => time::sys_clock_getres,
    RtSigaction = 134 => signal::sys_rt_sigaction,
    RtSigprocmask = 135 => signal::sys_rt_sigprocmask,
    SetGid = 144 => generic::proc::sys_setgid,
    SetUid = 146 => generic::proc::sys_setuid,
    SetPgid = 154 => generic::proc::sys_setpgid,
    GetPgid = 155 => generic::proc::sys_getpgid,
    Uname = 160 => proc::sys_uname,
    Umask = 166 => fs::sys_umask,
    GetPid = 172 => generic::proc::sys_getpid,
    GetPpid = 173 => generic::proc::sys_getppid,
    GetUid = 174 => generic::proc::sys_getuid,
    GetEuid = 175 => generic::proc::sys_geteuid,
    GetGid = 176 => generic::proc::sys_getgid,
    GetEgid = 177 => generic::proc::sys_getegid,
    GetTid = 178 => generic::proc::sys_gettid,
    Brk = 214 => generic::proc::sys_brk,
    Munmap = 215 => mm::sys_munmap,
    Clone = 220 => generic::proc::sys_clone,
    Execve = 221 => fs::sys_execve,
    Mmap = 222 => mm::sys_mmap,
    Mprotect = 226 => mm::sys_mprotect,
    EpollWait = 232 => fs::sys_epoll_wait,
    Wait4 = 260 => generic::proc::sys_wait4,
    Prlimit64 = 261 => generic::proc::sys_prlimit64,
    Socket = 198 => socket::sys_socket,
    Bind = 200 => socket::sys_bind,
    Listen = 201 => socket::sys_listen,
    Accept = 202 => socket::sys_accept,
    Connect = 203 => socket::sys_connect,
    GetSockname = 204 => socket::sys_getsockname,
    SetSockopt = 208 => socket::sys_setsockopt,
    GetSockopt = 209 => socket::sys_getsockopt,
    RenameAt2 = 276 => fs::sys_renameat2,
    Membarrier = 283 => generic::proc::sys_membarrier,
    ProcessMrelease = 448 => generic::proc::sys_process_mrelease,
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
    register_abi!(LinuxAarch64Abi);
}

early_initcall!(register_linux_abi);
