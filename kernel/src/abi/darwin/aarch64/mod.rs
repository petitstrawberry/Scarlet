//! Darwin ABI module for AArch64

mod bsd_syscalls;
mod mach_syscalls;
mod macho_loader;
mod syscall_table;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::Ordering;
use hashbrown::HashMap;

#[allow(unused_imports)]
use crate::ipc::event::{Event, EventContent, EventPriority, ProcessControlType};
use crate::{
    abi::AbiModule,
    arch::Trapframe,
    fs::{
        FileSystemError, FileSystemErrorKind, SeekFrom, VfsManager, drivers::overlayfs::OverlayFS,
    },
    late_initcall, register_abi,
    task::{CloneFlags, mytask},
};

const MAX_FDS: usize = 1024;
const MACH_PORT_NULL: u32 = 0;

// Darwin signal numbers (matching macOS / XNU definitions)
#[allow(dead_code)]
const SIGHUP: usize = 1;
#[allow(dead_code)]
const SIGINT: usize = 2;
#[allow(dead_code)]
const SIGQUIT: usize = 3;
#[allow(dead_code)]
const SIGILL: usize = 4;
#[allow(dead_code)]
const SIGTRAP: usize = 5;
#[allow(dead_code)]
const SIGABRT: usize = 6;
#[allow(dead_code)]
const SIGKILL: usize = 9;
#[allow(dead_code)]
const SIGBUS: usize = 10;
#[allow(dead_code)]
const SIGSEGV: usize = 11;
#[allow(dead_code)]
const SIGPIPE: usize = 13;
#[allow(dead_code)]
const SIGALRM: usize = 14;
#[allow(dead_code)]
const SIGTERM: usize = 15;
#[allow(dead_code)]
const SIGSTOP: usize = 17;
#[allow(dead_code)]
const SIGCONT: usize = 19;
#[allow(dead_code)]
const SIGCHLD: usize = 20;
#[allow(dead_code)]
const SIGUSR1: usize = 30;
#[allow(dead_code)]
const SIGUSR2: usize = 31;

const SIGNAL_FRAME_SIZE: usize = 4096;
const SIGNAL_TRAMPOLINE_OFFSET: usize = 0;
const SIGNAL_SAVED_REGS_OFFSET: usize = 16;
const SIGNAL_SAVED_SP_OFFSET: usize = SIGNAL_SAVED_REGS_OFFSET + 31 * 8;
const SIGNAL_SAVED_PC_OFFSET: usize = SIGNAL_SAVED_SP_OFFSET + 8;
const SIGNAL_SAVED_PSTATE_OFFSET: usize = SIGNAL_SAVED_PC_OFFSET + 8;
const SIGNAL_FRAME_USED_SIZE: usize = SIGNAL_SAVED_PSTATE_OFFSET + 8;

#[derive(Clone)]
pub struct DarwinAarch64Abi {
    fd_to_handle: Vec<Option<u32>>,
    free_fds: Vec<usize>,
    mach_task_port: u32,
    mach_thread_port: u32,
    next_mach_port: u32,
    mach_ports: HashMap<u32, MachPortInfo>,
    last_mach_message: Option<MachMessageBuffer>,
    /// Signal handler table: signal number -> handler address (0 = default)
    signal_handlers: [usize; 32],
    /// Signal mask: bit N set = signal N is blocked
    signal_mask: u32,
    /// Pending signals
    pending_signals: u32,
}

#[derive(Clone, Debug)]
struct MachPortInfo {
    right: MachPortRight,
}

#[derive(Clone, Debug)]
struct MachMessageBuffer {
    port_name: u32,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MachPortRight {
    Send,
    Receive,
    SendReceive,
    SendOnce,
    PortSet,
    Dead,
}

impl Default for DarwinAarch64Abi {
    fn default() -> Self {
        let mut free_fds: Vec<usize> = (0..MAX_FDS).collect();
        free_fds.reverse();

        Self {
            fd_to_handle: alloc::vec![None; MAX_FDS],
            free_fds,
            mach_task_port: 0x103,
            mach_thread_port: 0x307,
            next_mach_port: 0x309,
            mach_ports: HashMap::new(),
            last_mach_message: None,
            signal_handlers: [0; 32],
            signal_mask: 0,
            pending_signals: 0,
        }
    }
}

impl DarwinAarch64Abi {
    /// Map ProcessControlType to Darwin signal number
    fn process_control_to_signal(ptype: ProcessControlType) -> Option<usize> {
        match ptype {
            ProcessControlType::Terminate => Some(SIGTERM),
            ProcessControlType::Kill => Some(SIGKILL),
            ProcessControlType::Stop => Some(SIGSTOP),
            ProcessControlType::Continue => Some(SIGCONT),
            ProcessControlType::Interrupt => Some(SIGINT),
            ProcessControlType::Quit => Some(SIGQUIT),
            ProcessControlType::Hangup => Some(SIGHUP),
            ProcessControlType::ChildExit => Some(SIGCHLD),
            ProcessControlType::PipeBroken => Some(SIGPIPE),
            ProcessControlType::Alarm => Some(SIGALRM),
            ProcessControlType::IoReady => None,
            ProcessControlType::User(n) => {
                if n == 0 {
                    Some(SIGUSR1)
                } else {
                    Some(SIGUSR2)
                }
            }
        }
    }

    fn signal_bit(signal: usize) -> Option<u32> {
        if (1..32).contains(&signal) {
            Some(1u32 << signal)
        } else {
            None
        }
    }

    fn is_signal_blocked(&self, signal: usize) -> bool {
        Self::signal_bit(signal)
            .map(|bit| (self.signal_mask & bit) != 0)
            .unwrap_or(false)
    }

    fn mark_signal_pending(&mut self, signal: usize) {
        if let Some(bit) = Self::signal_bit(signal) {
            self.pending_signals |= bit;
        }
    }

    fn clear_pending_signal(&mut self, signal: usize) {
        if let Some(bit) = Self::signal_bit(signal) {
            self.pending_signals &= !bit;
        }
    }

    pub fn allocate_fd(&mut self, handle: u32) -> Result<usize, &'static str> {
        let fd = self.free_fds.pop().ok_or("Too many open files")?;
        self.fd_to_handle[fd] = Some(handle);
        Ok(fd)
    }

    pub fn allocate_specific_fd(&mut self, fd: usize, handle: u32) -> Result<(), &'static str> {
        if fd >= MAX_FDS {
            return Err("File descriptor out of range");
        }
        if self.fd_to_handle[fd].is_some() {
            return Err("File descriptor already in use");
        }
        if let Some(pos) = self.free_fds.iter().position(|&x| x == fd) {
            self.free_fds.remove(pos);
        }
        self.fd_to_handle[fd] = Some(handle);
        Ok(())
    }

    pub fn get_handle(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            self.fd_to_handle[fd]
        } else {
            None
        }
    }

    pub fn remove_fd(&mut self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            self.fd_to_handle[fd].take().map(|handle| {
                self.free_fds.push(fd);
                handle
            })
        } else {
            None
        }
    }

    pub fn mach_task_self(&self) -> usize {
        self.mach_task_port as usize
    }

    pub fn mach_thread_self(&self) -> usize {
        self.mach_thread_port as usize
    }

    pub fn allocate_mach_port(&mut self, right: u32) -> Result<u32, &'static str> {
        let port_name = self.next_mach_port;
        self.next_mach_port += 1;

        let port_right = match right {
            0 => MachPortRight::Send,
            1 => MachPortRight::Receive,
            2 => MachPortRight::SendReceive,
            3 => MachPortRight::SendOnce,
            4 => MachPortRight::PortSet,
            _ => MachPortRight::Send,
        };

        self.mach_ports
            .insert(port_name, MachPortInfo { right: port_right });
        Ok(port_name)
    }

    pub fn deallocate_mach_port(&mut self, name: u32) {
        self.mach_ports.remove(&name);
        if self
            .last_mach_message
            .as_ref()
            .is_some_and(|message| message.port_name == name)
        {
            self.last_mach_message = None;
        }
    }

    fn store_mach_message(&mut self, port_name: u32, data: Vec<u8>) {
        self.last_mach_message = Some(MachMessageBuffer { port_name, data });
    }

    fn take_mach_message(&mut self, port_name: u32) -> Option<MachMessageBuffer> {
        if self
            .last_mach_message
            .as_ref()
            .is_some_and(|message| message.port_name == port_name)
        {
            self.last_mach_message.take()
        } else {
            self.last_mach_message.take()
        }
    }

    fn dispatch_bsd_syscall(
        &mut self,
        num: u32,
        trapframe: &mut Trapframe,
    ) -> Result<usize, &'static str> {
        use syscall_table::*;
        match num {
            SYS_exit => {
                bsd_syscalls::sys_exit(self, trapframe);
                Ok(0)
            }
            SYS_fork => Ok(bsd_syscalls::sys_fork(self, trapframe)),
            SYS_read => Ok(bsd_syscalls::sys_read(self, trapframe)),
            SYS_write => Ok(bsd_syscalls::sys_write(self, trapframe)),
            SYS_open => Ok(bsd_syscalls::sys_open(self, trapframe)),
            SYS_close => Ok(bsd_syscalls::sys_close(self, trapframe)),
            SYS_getpid => Ok(bsd_syscalls::sys_getpid(self, trapframe)),
            SYS_getppid => Ok(bsd_syscalls::sys_getppid(self, trapframe)),
            SYS_getuid => Ok(bsd_syscalls::sys_getuid(self, trapframe)),
            SYS_geteuid => Ok(bsd_syscalls::sys_geteuid(self, trapframe)),
            SYS_getgid => Ok(bsd_syscalls::sys_getgid(self, trapframe)),
            SYS_getegid => Ok(bsd_syscalls::sys_getegid(self, trapframe)),
            SYS_socket => Ok(bsd_syscalls::sys_socket(self, trapframe)),
            SYS_bind => Ok(bsd_syscalls::sys_bind(self, trapframe)),
            SYS_connect => Ok(bsd_syscalls::sys_connect(self, trapframe)),
            SYS_listen => Ok(bsd_syscalls::sys_listen(self, trapframe)),
            SYS_accept => Ok(bsd_syscalls::sys_accept(self, trapframe)),
            SYS_sendto => Ok(bsd_syscalls::sys_sendto(self, trapframe)),
            SYS_recvfrom => Ok(bsd_syscalls::sys_recvfrom(self, trapframe)),
            SYS_shutdown => Ok(bsd_syscalls::sys_shutdown(self, trapframe)),
            SYS_dup => Ok(bsd_syscalls::sys_dup(self, trapframe)),
            SYS_dup2 => Ok(bsd_syscalls::sys_dup2(self, trapframe)),
            SYS_wait4 => Ok(bsd_syscalls::sys_wait4(self, trapframe)),
            SYS_sigaction => Ok(bsd_syscalls::sys_sigaction(self, trapframe)),
            SYS_sigprocmask => Ok(bsd_syscalls::sys_sigprocmask(self, trapframe)),
            SYS_sigreturn => Ok(bsd_syscalls::sys_sigreturn(self, trapframe)),
            SYS_fcntl => Ok(bsd_syscalls::sys_fcntl(self, trapframe)),
            SYS_ioctl => Ok(bsd_syscalls::sys_ioctl(self, trapframe)),
            SYS_lseek => Ok(bsd_syscalls::sys_lseek(self, trapframe)),
            SYS_pread => Ok(bsd_syscalls::sys_pread(self, trapframe)),
            SYS_getrlimit => Ok(bsd_syscalls::sys_getrlimit(self, trapframe)),
            SYS_setrlimit => Ok(bsd_syscalls::sys_setrlimit(self, trapframe)),
            SYS_fsgetpath => Ok(bsd_syscalls::sys_fsgetpath(self, trapframe)),
            SYS_mmap => Ok(bsd_syscalls::sys_mmap(self, trapframe)),
            SYS_mprotect => Ok(bsd_syscalls::sys_mprotect(self, trapframe)),
            SYS_munmap => Ok(bsd_syscalls::sys_munmap(self, trapframe)),
            SYS_thread_selfid => Ok(bsd_syscalls::sys_thread_selfid(self, trapframe)),
            SYS_access => Ok(bsd_syscalls::sys_access(self, trapframe)),
            SYS_proc_info => Ok(bsd_syscalls::sys_proc_info(self, trapframe)),
            SYS_execve => Ok(bsd_syscalls::sys_execve(self, trapframe)),
            SYS_shared_region_check_np => {
                Ok(bsd_syscalls::sys_shared_region_check_np(self, trapframe))
            }
            SYS_getentropy => Ok(bsd_syscalls::sys_getentropy(self, trapframe)),
            SYS_getlogin => Ok(bsd_syscalls::sys_getlogin(self, trapframe)),
            SYS_terminate_with_payload => {
                Ok(bsd_syscalls::sys_terminate_with_payload(self, trapframe))
            }
            SYS_abort_with_payload => Ok(bsd_syscalls::sys_abort_with_payload(self, trapframe)),
            SYS_stat_old => Ok(bsd_syscalls::sys_stat(self, trapframe)),
            SYS_csrctl => Ok(bsd_syscalls::sys_csrctl(self, trapframe)),
            SYS_ulock_wait => Ok(bsd_syscalls::sys_ulock_wait(self, trapframe)),
            SYS_ulock_wake => Ok(bsd_syscalls::sys_ulock_wake(self, trapframe)),
            SYS_ulock_wait2 => Ok(bsd_syscalls::sys_ulock_wait2(self, trapframe)),
            SYS_stat => Ok(bsd_syscalls::sys_stat(self, trapframe)),
            SYS_stat64 => Ok(bsd_syscalls::sys_stat(self, trapframe)),
            SYS_lstat => Ok(bsd_syscalls::sys_lstat(self, trapframe)),
            SYS_fstat => Ok(bsd_syscalls::sys_fstat(self, trapframe)),
            SYS_map_with_linking_np => {
                Ok(bsd_syscalls::sys_map_with_linking_np(self, trapframe))
            }
            SYS_fsgetpath_ext => Ok(bsd_syscalls::sys_fsgetpath(self, trapframe)),
            SYS_sysctl => Ok(bsd_syscalls::sys_sysctl(self, trapframe)),
            SYS___mac_syscall => Ok(bsd_syscalls::sys_mac_syscall(self, trapframe)),
            SYS_openat => Ok(bsd_syscalls::sys_openat(self, trapframe)),
            SYS_bsdthread_register => Ok(bsd_syscalls::sys_bsdthread_register(self, trapframe)),
            SYS___pthread_sigmask => Ok(bsd_syscalls::sys_pthread_sigmask(self, trapframe)),
            SYS___pthread_kill => Ok(bsd_syscalls::sys_pthread_kill(self, trapframe)),
            SYS___semwait_signal_nocancel => {
                Ok(bsd_syscalls::sys_semwait_signal_nocancel(self, trapframe))
            }
            _ => {
                crate::println!("[darwin] Unimplemented BSD syscall: {} (0x{:x})", num, num);
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(super::error::ENOSYS);
                Ok(usize::MAX)
            }
        }
    }

    fn handle_thread_set_tsd_base(
        &mut self,
        trapframe: &mut Trapframe,
    ) -> Result<usize, &'static str> {
        let tsd_base = trapframe.regs.reg[0];
        let task = mytask().unwrap();
        trapframe.increment_pc_next(task);

        trapframe.tpidr_el0 = tsd_base as u64;
        trapframe.tpidrro_el0 = tsd_base as u64;

        let mut vcpu = task.vcpu.lock();
        vcpu.set_tpidr_el0(tsd_base as u64);
        vcpu.set_tpidrro_el0(tsd_base as u64);

        // On real macOS, the kernel pre-populates TSD slot 3 (__TSD_MACH_THREAD_SELF)
        // with the thread's mach port name. dyld's _mach_init does NOT call mach_thread_self -
        // it relies on this pre-populated value for os_unfair_lock ownership checks.
        let thread_port_bytes = self.mach_thread_port.to_ne_bytes();
        let _ = crate::library::std::usercopy::copy_to_user(
            &task,
            tsd_base as usize + 0x18,
            &thread_port_bytes,
        );

        trapframe.set_return_value(0);
        Ok(0)
    }

    fn dispatch_mach_syscall(
        &mut self,
        num: i32,
        trapframe: &mut Trapframe,
    ) -> Result<usize, &'static str> {
        use syscall_table::*;
        match num {
            MACH_mach_reply_port => {
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                let port = self.next_mach_port;
                self.next_mach_port += 1;
                crate::println!("[darwin] mach_reply_port: {:#x}", port);
                trapframe.set_return_value(port as usize);
                Ok(port as usize)
            }
            MACH__kernelrpc_mach_port_allocate_trap => {
                Ok(mach_syscalls::sys_mach_port_allocate(self, trapframe))
            }
            MACH_task_for_pid => Ok(mach_syscalls::sys_task_for_pid(self, trapframe)),
            MACH__kernelrpc_mach_port_deallocate_trap => {
                Ok(mach_syscalls::sys_mach_port_deallocate(self, trapframe))
            }
            MACH_mach_msg_trap => Ok(mach_syscalls::sys_mach_msg_trap(self, trapframe)),
            MACH__kernelrpc_mach_vm_allocate_trap => {
                Ok(mach_syscalls::sys_vm_allocate(self, trapframe))
            }
            MACH__kernelrpc_mach_vm_deallocate_trap => {
                Ok(mach_syscalls::sys_vm_deallocate(self, trapframe))
            }
            MACH__kernelrpc_mach_vm_protect_trap => {
                Ok(mach_syscalls::sys_vm_protect(self, trapframe))
            }
            MACH_mach_timebase_info_trap => {
                Ok(mach_syscalls::sys_mach_timebase_info(self, trapframe))
            }
            MACH_thread_self_trap => {
                crate::println!("[darwin] mach_thread_self");
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                let port = self.mach_thread_self();
                trapframe.set_return_value(port);
                Ok(port)
            }
            MACH_task_self_trap => {
                crate::println!("[darwin] mach_task_self");
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                let port = self.mach_task_self();
                trapframe.set_return_value(port);
                Ok(port)
            }
            MACH__kernelrpc_mach_vm_map_trap => Ok(mach_syscalls::sys_vm_map(self, trapframe)),
            MACH__kernelrpc_mach_port_mod_refs_trap => {
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                trapframe.set_return_value(super::error::KERN_SUCCESS as usize);
                Ok(0)
            }
            MACH_host_self_trap => {
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                let port = self.next_mach_port;
                self.next_mach_port += 1;
                trapframe.set_return_value(port as usize);
                Ok(port as usize)
            }
            MACH__kernelrpc_mach_port_construct_trap => {
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                let name_ptr = trapframe.regs.reg[0];
                let port = self.next_mach_port;
                self.next_mach_port += 1;
                if name_ptr != 0 {
                    if let Some(kva) = task.vm_manager.translate_to_kva(name_ptr) {
                        unsafe { (kva as *mut u32).write(port as u32) };
                    }
                }
                trapframe.set_return_value(super::error::KERN_SUCCESS as usize);
                Ok(0)
            }
            MACH_mach_msg2_trap => Ok(mach_syscalls::sys_mach_msg2_trap(self, trapframe)),
            MACH_thread_set_tsd_base => self.handle_thread_set_tsd_base(trapframe),
            // ARM64 fast traps (not in mach_trap_table, small negative numbers)
            FAST_MACH_absolute_time => {
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                let t = crate::arch::aarch64::timer::get_time();
                trapframe.regs.reg[0] = t as usize;
                Ok(t as usize)
            }
            FAST_MACH_continuous_time => {
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                let t = crate::arch::aarch64::timer::get_time();
                trapframe.regs.reg[0] = t as usize;
                Ok(t as usize)
            }
            _ => {
                crate::println!("[darwin] Unimplemented Mach trap: {}", num);
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                trapframe.set_return_value(super::error::KERN_FAILURE as usize);
                Ok(usize::MAX)
            }
        }
    }

    /// Setup argc, argv, and envp on the user stack following Unix conventions
    ///
    /// Darwin dyld stack layout (from high to low addresses):
    /// ```
    /// [high addresses]
    /// apple strings (null-terminated)
    /// envp strings (null-terminated)
    /// argv strings (null-terminated)
    /// apple[] array (null-terminated pointer array)
    /// envp[] array (null-terminated pointer array)
    /// argv[] array (null-terminated pointer array)
    /// argc (integer)
    /// mach_header pointer
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
        task: &crate::task::Task,
        argv: &[&str],
        envp: &[&str],
        initial_sp: usize,
        mach_header_addr: usize,
        executable_path: &str,
    ) -> Result<(usize, usize), &'static str> {
        let argc = argv.len();
        let envc = envp.len();

        let argv_strings_size: usize = argv.iter().map(|s| s.len() + 1).sum();
        let envp_strings_size: usize = envp.iter().map(|s| s.len() + 1).sum();

        let argv_array_size = (argc + 1) * core::mem::size_of::<usize>();
        let envp_array_size = (envc + 1) * core::mem::size_of::<usize>();
        let argc_size = core::mem::size_of::<usize>();

        let exe_apple_str = alloc::format!("executable_path={}", executable_path);
        let dyld_file_str = "dyld_file=0x1,0x1";
        let apple_strings_size = exe_apple_str.len() + 1 + dyld_file_str.len() + 1;
        let apple_array_size = 3 * core::mem::size_of::<usize>(); // 2 pointers + NULL

        let total_size = core::mem::size_of::<usize>() // mach_header* at SP+0
            + argc_size + argv_array_size + envp_array_size
            + apple_array_size + argv_strings_size + envp_strings_size
            + apple_strings_size;

        let aligned_total_size = (total_size + 15) & !15;
        let new_sp = initial_sp - aligned_total_size;

        let mut current_addr = new_sp;

        // 0. Write mach_header pointer at SP+0. dyld's KernelArgs helpers
        // pre-increment by one word before reading argc, so the stack must
        // start with the main executable mach_header followed by argc.
        self.write_to_stack_memory(task, current_addr, &mach_header_addr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();

        // 1. Write argc.
        self.write_to_stack_memory(task, current_addr, &argc.to_le_bytes())?;
        current_addr += argc_size;

        // 2. Save argv array pointer for return value
        let argv_ptr = current_addr;

        // 3. Calculate string positions
        let argv_strings_start =
            current_addr + argv_array_size + envp_array_size + apple_array_size;
        let envp_strings_start = argv_strings_start + argv_strings_size;
        let apple_strings_start = envp_strings_start + envp_strings_size;

        // 4. Write argv[] array
        let mut string_addr = argv_strings_start;
        for i in 0..argc {
            self.write_to_stack_memory(task, current_addr, &string_addr.to_le_bytes())?;
            current_addr += core::mem::size_of::<usize>();
            string_addr += argv[i].len() + 1;
        }
        let null_ptr: usize = 0;
        self.write_to_stack_memory(task, current_addr, &null_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();

        // 5. Write envp[] array
        string_addr = envp_strings_start;
        for i in 0..envc {
            self.write_to_stack_memory(task, current_addr, &string_addr.to_le_bytes())?;
            current_addr += core::mem::size_of::<usize>();
            string_addr += envp[i].len() + 1;
        }
        self.write_to_stack_memory(task, current_addr, &null_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();

        // 6. Write apple[] array
        self.write_to_stack_memory(task, current_addr, &apple_strings_start.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();
        let dyld_file_ptr = apple_strings_start + exe_apple_str.len() + 1;
        self.write_to_stack_memory(task, current_addr, &dyld_file_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();
        self.write_to_stack_memory(task, current_addr, &null_ptr.to_le_bytes())?;
        current_addr += core::mem::size_of::<usize>();

        // 7. Write argv strings
        for arg in argv {
            self.write_string_to_stack(task, current_addr, arg)?;
            current_addr += arg.len() + 1;
        }

        // 8. Write envp strings
        for env in envp {
            self.write_string_to_stack(task, current_addr, env)?;
            current_addr += env.len() + 1;
        }

        // 9. Write apple strings
        self.write_string_to_stack(task, current_addr, &exe_apple_str)?;
        current_addr += exe_apple_str.len() + 1;
        self.write_string_to_stack(task, current_addr, dyld_file_str)?;
        current_addr += dyld_file_str.len() + 1;

        Ok((new_sp, argv_ptr))
    }

    /// Write bytes to stack memory using virtual memory translation
    fn write_to_stack_memory(
        &self,
        task: &crate::task::Task,
        vaddr: usize,
        data: &[u8],
    ) -> Result<(), &'static str> {
        let mut written = 0usize;
        while written < data.len() {
            let current_vaddr = vaddr + written;
            let page_off = current_vaddr & (crate::environment::PAGE_SIZE - 1);
            let chunk_len = core::cmp::min(
                data.len() - written,
                crate::environment::PAGE_SIZE - page_off,
            );

            match task.vm_manager.translate_to_kva(current_vaddr) {
                Some(paddr) => {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data[written..written + chunk_len].as_ptr(),
                            paddr as *mut u8,
                            chunk_len,
                        );
                    }
                    written += chunk_len;
                }
                None => return Err("Failed to translate virtual address for stack write"),
            }
        }

        Ok(())
    }

    /// Write a null-terminated string to stack memory
    fn write_string_to_stack(
        &self,
        task: &crate::task::Task,
        vaddr: usize,
        string: &str,
    ) -> Result<(), &'static str> {
        // Write the string content
        self.write_to_stack_memory(task, vaddr, string.as_bytes())?;
        // Write null terminator
        self.write_to_stack_memory(task, vaddr + string.len(), &[0u8])?;
        Ok(())
    }

    /// Read a usize value from stack memory (for verification)
    fn read_usize_from_stack(
        &self,
        task: &crate::task::Task,
        vaddr: usize,
    ) -> Option<usize> {
        match task.vm_manager.translate_to_kva(vaddr) {
            Some(kva) => {
                let ptr = kva as *const u8;
                let mut buf = [0u8; 8];
                // SAFETY: translate_to_kva returned a valid kernel virtual address
                unsafe {
                    core::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), 8);
                }
                Some(usize::from_le_bytes(buf))
            }
            None => None,
        }
    }

    /// Read a null-terminated string from stack memory (for verification)
    fn read_string_from_stack(
        &self,
        task: &crate::task::Task,
        vaddr: usize,
        max_len: usize,
    ) -> Option<alloc::string::String> {
        let mut s = alloc::string::String::new();
        let mut addr = vaddr;
        for _ in 0..max_len {
            match task.vm_manager.translate_to_kva(addr) {
                Some(kva) => {
                    // SAFETY: translate_to_kva returned a valid kernel virtual address
                    let byte = unsafe { *(kva as *const u8) };
                    if byte == 0 {
                        return Some(s);
                    }
                    s.push(byte as char);
                    addr += 1;
                }
                None => return None,
            }
        }
        Some(s)
    }

    /// Verify the stack layout by walking it exactly like dyld's KernelArgs does
    fn verify_stack_layout(
        &self,
        task: &crate::task::Task,
        sp: usize,
    ) {
        crate::println!("[darwin] === STACK VERIFY at SP={:#x} ===", sp);

        // Read mach_header at SP+0
        let mh = self.read_usize_from_stack(task, sp);
        crate::println!("[darwin]   SP+0 mach_header = {:#x}", mh.unwrap_or(0xDEAD));

        // Read argc at SP+8
        let argc = self.read_usize_from_stack(task, sp + 8);
        crate::println!("[darwin]   SP+8 argc = {:?}", argc);

        let mut offset = 16; // past mach_header + argc
        let ac = argc.unwrap_or(0);

        // Walk argv[]
        crate::println!("[darwin]   --- argv ---");
        for i in 0..ac {
            let ptr = self.read_usize_from_stack(task, sp + offset);
            crate::println!("[darwin]     argv[{}] ptr={:#x}", i, ptr.unwrap_or(0));
            if let Some(p) = ptr {
                if p != 0 {
                    if let Some(s) = self.read_string_from_stack(task, p, 256) {
                        crate::println!("[darwin]     argv[{}] = \"{}\"", i, s);
                    }
                }
            }
            offset += 8;
        }
        let argv_null = self.read_usize_from_stack(task, sp + offset);
        crate::println!("[darwin]     argv NULL = {:#x}", argv_null.unwrap_or(0xBAD));
        offset += 8;

        // Walk envp[]
        crate::println!("[darwin]   --- envp ---");
        let mut env_idx = 0;
        loop {
            let ptr = self.read_usize_from_stack(task, sp + offset);
            if ptr.unwrap_or(0xBAD) == 0 {
                crate::println!("[darwin]     envp NULL terminator at offset +{}", offset);
                offset += 8;
                break;
            }
            crate::println!("[darwin]     envp[{}] ptr={:#x}", env_idx, ptr.unwrap_or(0));
            if let Some(p) = ptr {
                if let Some(s) = self.read_string_from_stack(task, p, 256) {
                    crate::println!("[darwin]     envp[{}] = \"{}\"", env_idx, s);
                }
            }
            offset += 8;
            env_idx += 1;
            if env_idx > 20 {
                crate::println!("[darwin]     envp: too many entries, stopping");
                break;
            }
        }

        // Walk apple[]
        crate::println!("[darwin]   --- apple ---");
        let mut apple_idx = 0;
        loop {
            let ptr = self.read_usize_from_stack(task, sp + offset);
            if ptr.unwrap_or(0xBAD) == 0 {
                crate::println!("[darwin]     apple NULL terminator at offset +{}", offset);
                offset += 8;
                break;
            }
            crate::println!("[darwin]     apple[{}] ptr={:#x}", apple_idx, ptr.unwrap_or(0));
            if let Some(p) = ptr {
                if let Some(s) = self.read_string_from_stack(task, p, 256) {
                    crate::println!("[darwin]     apple[{}] = \"{}\"", apple_idx, s);
                }
            }
            offset += 8;
            apple_idx += 1;
            if apple_idx > 10 {
                crate::println!("[darwin]     apple: too many entries, stopping");
                break;
            }
        }

        crate::println!("[darwin] === END STACK VERIFY ===");
    }

    fn execute_binary_with_path(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut Trapframe,
        executable_path: &str,
    ) -> Result<(), &'static str> {
        let file_obj = file_object.as_file().ok_or("Invalid file object type")?;

        task.text_size.store(0, Ordering::SeqCst);
        task.data_size.store(0, Ordering::SeqCst);
        task.stack_size.store(0, Ordering::SeqCst);
        task.brk.store(usize::MAX, Ordering::SeqCst);

        crate::println!("[darwin] execute_binary: loading Mach-O...");
        let (entry_point, dyld_path, mach_header_addr) =
            macho_loader::load_macho_binary(file_obj, task).map_err(|e| {
                crate::println!("[darwin] load_macho_binary FAILED: {}", e);
                e
            })?;
        crate::println!(
            "[darwin] macho OK: entry={:#x} dyld={:?} mh={:#x}",
            entry_point,
            dyld_path,
            mach_header_addr
        );

        if let Some(dyld) = dyld_path {
            let vfs = task.get_vfs().ok_or("No VFS")?;
            macho_loader::ensure_shared_cache(&vfs)?;

            crate::println!("[darwin] loading dyld from '{}'...", dyld);
            let (dyld_entry, _base_delta) = macho_loader::load_dyld(&dyld, task).map_err(|e| {
                crate::println!("[darwin] load_dyld FAILED: {}", e);
                e
            })?;
            crate::println!(
                "[darwin] dyld OK: entry={:#x} delta={:#x}",
                dyld_entry,
                _base_delta
            );

            macho_loader::setup_shared_cache_region(task).map_err(|e| {
                crate::println!("[darwin] setup_shared_cache_region FAILED: {}", e);
                e
            })?;

            macho_loader::setup_commpage(task).map_err(|e| {
                crate::println!("[darwin] setup_commpage FAILED: {}", e);
                e
            })?;

            *task.name.write() = argv
                .first()
                .map_or("Unnamed Darwin Task".to_string(), |s| s.to_string());

            let root_page_table =
                crate::arch::vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
            root_page_table.unmap_all();

            crate::arch::vm::setup_trampoline_for_user(&task.vm_manager);
            let stack_pointer = crate::vm::setup_user_stack(task).1;

            task.vcpu.lock().reset_iregs();
            task.vcpu.lock().set_sp(stack_pointer);

            let (adjusted_sp, _argv_ptr) = self.setup_arguments_on_stack(
                task,
                argv,
                envp,
                stack_pointer,
                mach_header_addr,
                executable_path,
            )?;

            self.verify_stack_layout(task, adjusted_sp);

            // dyld arm64 convention:
            //   __dyld_start copies SP into x0 and passes that KernelArgs* to
            //   dyld::start().  The stack starts with [mach_header, argc,
            //   argv..., NULL, envp..., NULL, apple..., NULL, strings].

            // Pre-allocate TLS page with TSD slot 3 (__TSD_MACH_THREAD_SELF) set to
            // mach_thread_port. On real macOS, the kernel populates this before the
            // first user instruction so that os_unfair_lock ownership checks work
            // from the very start. Without this, TPIDR_EL0=0 causes os_unfair_lock_lock
            // to read [0+0x18] as the "owner", producing a garbage value that later
            // mismatches the correct TSD[3]=0x307 set by thread_set_tsd_base.
            let tls_vaddr = macho_loader::setup_tls(task, self.mach_thread_port).map_err(|e| {
                crate::println!("[darwin] setup_tls FAILED: {}", e);
                e
            })?;

            macho_loader::setup_comm_page(task).map_err(|e| {
                crate::println!("[darwin] setup_comm_page FAILED: {}", e);
                e
            })?;

            task.set_entry_point(dyld_entry);
            {
                let mut vcpu = task.vcpu.lock();
                vcpu.set_sp(adjusted_sp);
                vcpu.set_pc(dyld_entry as u64);
                // Set TPIDR_EL0/TPIDRRO_EL0 to the pre-allocated TLS page so that
                // TSD slot access (TPIDRRO_EL0 + offset) works from instruction one.
                vcpu.set_tpidr_el0(tls_vaddr as u64);
                vcpu.set_tpidrro_el0(tls_vaddr as u64);
                vcpu.switch(trapframe);
            }

            return Ok(());
        }

        *task.name.write() = argv
            .first()
            .map_or("Unnamed Darwin Task".to_string(), |s| s.to_string());

        let root_page_table =
            crate::arch::vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
        root_page_table.unmap_all();

        crate::arch::vm::setup_trampoline_for_user(&task.vm_manager);
        let stack_pointer = crate::vm::setup_user_stack(task).1;

        task.set_entry_point(entry_point);

        task.vcpu.lock().reset_iregs();
        task.vcpu.lock().set_sp(stack_pointer);

        let (adjusted_sp, argv_ptr) = self.setup_arguments_on_stack(
            task,
            argv,
            envp,
            stack_pointer,
            mach_header_addr,
            executable_path,
        )?;
        task.vcpu.lock().set_sp(adjusted_sp);

        task.vcpu.lock().iregs.reg[0] = argv.len();
        task.vcpu.lock().iregs.reg[1] = argv_ptr;

        task.vcpu.lock().switch(trapframe);
        Ok(())
    }
}

/// Set up signal frame on user stack and modify trapframe to call handler.
/// On Darwin AArch64, the signal handler prototype is:
///   void handler(int sig, siginfo_t *info, ucontext_t *ctx)
fn setup_signal_frame(
    task: &crate::task::Task,
    trapframe: &mut Trapframe,
    _abi: &DarwinAarch64Abi,
    signal: usize,
    handler: usize,
) -> Result<(), &'static str> {
    let sp = (trapframe.sp as usize) & !0xF;
    let frame_sp = (sp
        .checked_sub(SIGNAL_FRAME_SIZE)
        .ok_or("Signal frame stack underflow")?)
        & !0xF;

    if task.vm_manager.translate_to_kva(frame_sp).is_none()
        || task
            .vm_manager
            .translate_to_kva(frame_sp + SIGNAL_FRAME_USED_SIZE - 1)
            .is_none()
    {
        return Err("Failed to translate signal frame address");
    }

    unsafe {
        let trampoline =
            task.vm_manager
                .translate_to_kva(frame_sp + SIGNAL_TRAMPOLINE_OFFSET)
                .ok_or("Failed to translate signal frame address")? as *mut u32;
        // mov x0, sp
        *trampoline = 0x910003e0;
        // mov x16, #184
        *trampoline.add(1) = 0xd2801710;
        // movk x16, #0x200, lsl #16
        *trampoline.add(2) = 0xf2a04010;
        // svc #0x80
        *trampoline.add(3) = 0xd4001001;

        for i in 0..31 {
            let kva = task
                .vm_manager
                .translate_to_kva(frame_sp + SIGNAL_SAVED_REGS_OFFSET + i * 8)
                .ok_or("Failed to translate signal frame address")?;
            *(kva as *mut u64) = trapframe.regs.reg[i] as u64;
        }

        let saved_sp = task
            .vm_manager
            .translate_to_kva(frame_sp + SIGNAL_SAVED_SP_OFFSET)
            .ok_or("Failed to translate signal frame address")?;
        *(saved_sp as *mut u64) = trapframe.sp;

        let saved_pc = task
            .vm_manager
            .translate_to_kva(frame_sp + SIGNAL_SAVED_PC_OFFSET)
            .ok_or("Failed to translate signal frame address")?;
        *(saved_pc as *mut u64) = trapframe.elr;

        let saved_pstate = task
            .vm_manager
            .translate_to_kva(frame_sp + SIGNAL_SAVED_PSTATE_OFFSET)
            .ok_or("Failed to translate signal frame address")?;
        *(saved_pstate as *mut u64) = trapframe.spsr;
    }

    trapframe.regs.reg[0] = signal;
    trapframe.regs.reg[1] = 0;
    trapframe.regs.reg[2] = frame_sp;
    trapframe.regs.reg[30] = frame_sp + SIGNAL_TRAMPOLINE_OFFSET;
    trapframe.elr = handler as u64;
    trapframe.sp = frame_sp as u64;

    Ok(())
}

impl AbiModule for DarwinAarch64Abi {
    fn name() -> &'static str {
        "darwin-aarch64"
    }

    fn get_name(&self) -> String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }

    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        let svc_imm = (trapframe.esr_el1 & 0xFFFF) as u16;
        let syscall_num = trapframe.regs.reg[16] as u32;

        match svc_imm {
            // macOS ARM64: svc #0x80 for all syscalls.
            // x16 sign determines type: negative = Mach trap, positive = BSD syscall.
            0x80 => {
                let syscall_num_signed = syscall_num as i32;

                // Probe TSD[3] and dyld lock from user memory
                let task = crate::task::mytask().unwrap();
                let tpidrro = trapframe.tpidrro_el0 as usize;
                let tsd3 = if tpidrro > 0x1000 {
                    task.vm_manager
                        .translate_to_kva(tpidrro + 0x18)
                        .map(|kva| unsafe { *(kva as *const u32) })
                        .unwrap_or(0xDEAD)
                } else {
                    0
                };
                let lock_addr = 0x40000000 + 0xa9150;
                let lock_val = task
                    .vm_manager
                    .translate_to_kva(lock_addr)
                    .map(|kva| unsafe { *(kva as *const u32) })
                    .unwrap_or(0xDEAD);
                crate::println!(
                    "[darwin] probe: pc={:#x} x16={:#x} tsd3={:#x} lock={:#x} num={}",
                    trapframe.elr,
                    trapframe.regs.reg[16],
                    tsd3,
                    lock_val,
                    syscall_num,
                );

                if syscall_num_signed >= 0 {
                    trapframe.spsr &= !(1 << 29);
                }

                let result = if syscall_num_signed < 0 {
                    self.dispatch_mach_syscall(syscall_num_signed, trapframe)
                } else {
                    self.dispatch_bsd_syscall(syscall_num & 0xFFFFFF, trapframe)
                };
                let bsd_num = syscall_num & 0xFFFFFF;
                let carry = (trapframe.spsr >> 29) & 1;
                let task2 = crate::task::mytask().unwrap();
                let lock_after = task2
                    .vm_manager
                    .translate_to_kva(0x40000000 + 0xa9150)
                    .map(|kva| unsafe { *(kva as *const u32) })
                    .unwrap_or(0xDEAD);
                crate::println!(
                    "[darwin] result: x0={:#x} carry={} lock_after={:#x} x2={:#x} num={}",
                    trapframe.get_return_value(),
                    carry,
                    lock_after,
                    trapframe.regs.reg[2],
                    bsd_num,
                );
                if bsd_num == syscall_table::SYS_getentropy && lock_after == 0x307 {
                    crate::arch::aarch64::trap::exception::enable_lock_page_watch(0x400a9150);
                }
                if syscall_num_signed < 0 {
                    if result.is_ok() {
                        trapframe.spsr &= !(1 << 29);
                    } else {
                        trapframe.spsr |= 1 << 29;
                    }
                }
                result
            }
            0x81 => {
                let mach_num = syscall_num as i32;
                crate::println!(
                    "[darwin] mach81 trap: {:#x} tpidr={:#x} ro={:#x}",
                    mach_num,
                    trapframe.tpidr_el0,
                    trapframe.tpidrro_el0
                );
                let result = self.dispatch_mach_syscall(mach_num, trapframe);
                crate::println!(
                    "[darwin] mach81 result: x0={:#x} tpidr={:#x} ro={:#x}",
                    trapframe.get_return_value(),
                    trapframe.tpidr_el0,
                    trapframe.tpidrro_el0
                );
                if result.is_ok() {
                    trapframe.spsr &= !(1 << 29);
                }
                result
            }
            _ => {
                crate::println!(
                    "[darwin] Unknown SVC immediate: {} (ESR={:#x})",
                    svc_imm,
                    trapframe.esr_el1
                );
                Err("Unknown SVC immediate for Darwin ABI")
            }
        }
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        _file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                let mut magic_buffer = [0u8; 4];
                file_obj.seek(SeekFrom::Start(0)).ok()?;
                match file_obj.read(&mut magic_buffer) {
                    Ok(n) if n >= 4 => {
                        if magic_buffer == [0xCF, 0xFA, 0xED, 0xFE] {
                            40
                        } else if magic_buffer == [0xCE, 0xFA, 0xED, 0xFE] {
                            20
                        } else if magic_buffer == [0xCA, 0xFE, 0xBA, 0xBE] {
                            30
                        } else {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            None => return None,
        };

        let mut confidence = magic_score;

        if let Some(file_obj) = file_object.as_file() {
            let mut cpu_buf = [0u8; 4];
            file_obj.seek(SeekFrom::Start(4)).ok()?;
            if file_obj.read(&mut cpu_buf).ok()? >= 4 {
                let cputype = u32::from_le_bytes(cpu_buf);
                if cputype == 0x0100000C {
                    confidence += 40;
                }
            }
        }

        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 20;
            }
        }

        Some(confidence.min(100))
    }

    fn execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        argv: &[&str],
        envp: &[&str],
        task: &crate::task::Task,
        trapframe: &mut Trapframe,
    ) -> Result<(), &'static str> {
        self.execute_binary_with_path(
            file_object,
            argv,
            envp,
            task,
            trapframe,
            argv.first().copied().unwrap_or(""),
        )
    }

    fn get_default_cwd(&self) -> &str {
        "/"
    }

    fn setup_overlay_environment(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
        system_path: &str,
        config_path: &str,
    ) -> Result<(), &'static str> {
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
                    "Failed to create overlay filesystem for Darwin ABI: {}",
                    e.message
                );
                return Err("Failed to create Darwin overlay environment");
            }
        };

        match target_vfs.mount(fs, "/", 0) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::println!(
                    "Failed to create cross-VFS overlay for Darwin ABI: {}",
                    e.message
                );
                Err("Failed to create Darwin overlay environment")
            }
        }
    }

    fn setup_shared_resources(
        &self,
        target_vfs: &Arc<VfsManager>,
        base_vfs: &Arc<VfsManager>,
    ) -> Result<(), &'static str> {
        create_dir_if_not_exists(target_vfs, "/home").map_err(|e| {
            crate::println!("Failed to create /home directory for Darwin: {}", e.message);
            "Failed to create /home directory for Darwin"
        })?;
        let _ = target_vfs.bind_mount_from(base_vfs, "/home", "/home");

        create_dir_if_not_exists(target_vfs, "/data").map_err(|e| {
            crate::println!("Failed to create /data directory for Darwin: {}", e.message);
            "Failed to create /data directory for Darwin"
        })?;
        let _ = target_vfs.bind_mount_from(base_vfs, "/data/shared", "/data/shared");

        create_dir_if_not_exists(target_vfs, "/dev").map_err(|e| {
            crate::println!("Failed to create /dev directory for Darwin: {}", e.message);
            "Failed to create /dev directory for Darwin"
        })?;
        target_vfs
            .bind_mount_from(base_vfs, "/dev", "/dev")
            .map_err(|e| {
                crate::println!("Failed to bind mount /dev for Darwin: {}", e.message);
                "Failed to bind mount /dev for Darwin"
            })?;

        create_dir_if_not_exists(target_vfs, "/tmp").map_err(|e| {
            crate::println!("Failed to create /tmp directory for Darwin: {}", e.message);
            "Failed to create /tmp directory for Darwin"
        })?;
        target_vfs
            .bind_mount_from(base_vfs, "/tmp", "/tmp")
            .map_err(|e| {
                crate::println!("Failed to bind mount /tmp for Darwin: {}", e.message);
                "Failed to bind mount /tmp for Darwin"
            })?;

        create_dir_if_not_exists(target_vfs, "/scarlet").map_err(|e| {
            crate::println!(
                "Failed to create /scarlet directory for Darwin: {}",
                e.message
            );
            "Failed to create /scarlet directory for Darwin"
        })?;
        target_vfs
            .bind_mount_from(base_vfs, "/", "/scarlet")
            .map_err(|e| {
                crate::println!(
                    "Failed to bind mount Scarlet root to /scarlet for Darwin: {}",
                    e.message
                );
                "Failed to bind mount Scarlet root to /scarlet for Darwin"
            })?;

        create_dir_if_not_exists(target_vfs, "/Users").ok();

        Ok(())
    }

    fn on_task_cloned(
        &mut self,
        _parent_task: &crate::task::Task,
        _child_task: &crate::task::Task,
        _flags: CloneFlags,
    ) -> Result<(), &'static str> {
        // DarwinAarch64Abi derives Clone, so the ABI state (fd_to_handle, mach_ports, etc.)
        // is already cloned by clone_boxed(). The kernel handles the actual task cloning.
        // No additional work needed at this time.
        Ok(())
    }

    fn on_task_exit(&mut self, _task: &crate::task::Task) {
        // Darwin ABI cleanup on task exit:
        // - FD table and handle table are cleaned up by the kernel's generic task exit
        // - Mach ports are cleaned up when the ABI instance is dropped
        // Minimal cleanup needed for now.
    }

    fn handle_event(&mut self, event: Event, _target_task_id: u32) -> Result<(), &'static str> {
        let task = match crate::task::mytask() {
            Some(t) => t,
            None => return Err("No current task to handle event"),
        };

        let _priority = match event.metadata.priority {
            EventPriority::Low => EventPriority::Low,
            EventPriority::Normal => EventPriority::Normal,
            EventPriority::High => EventPriority::High,
            EventPriority::Critical => EventPriority::Critical,
        };

        match &event.content {
            EventContent::ProcessControl(ptype) => {
                let signal = Self::process_control_to_signal(*ptype);
                match signal {
                    Some(sig)
                        if self.is_signal_blocked(sig) && sig != SIGKILL && sig != SIGSTOP =>
                    {
                        self.mark_signal_pending(sig);
                        Ok(())
                    }
                    Some(SIGKILL) | Some(SIGTERM) => {
                        if let Some(sig) = signal {
                            self.clear_pending_signal(sig);
                        }
                        let exit_code = match signal {
                            Some(SIGKILL) => 128 + 9,
                            Some(SIGTERM) => 128 + 15,
                            _ => 1,
                        };
                        task.exit(exit_code);
                        Ok(())
                    }
                    Some(SIGSTOP) => {
                        self.clear_pending_signal(SIGSTOP);
                        task.set_state(crate::task::TaskState::Blocked(
                            crate::task::BlockedType::Interruptible,
                        ));
                        Ok(())
                    }
                    Some(SIGCONT) => {
                        self.clear_pending_signal(SIGCONT);
                        let current_state = task.get_state();
                        if matches!(current_state, crate::task::TaskState::Blocked(_)) {
                            task.set_state(crate::task::TaskState::Ready);
                        }
                        Ok(())
                    }
                    Some(sig) => {
                        self.clear_pending_signal(sig);
                        let sig_idx = sig.min(31);
                        if sig_idx > 0 && self.signal_handlers[sig_idx] != 0 {
                            let handler = self.signal_handlers[sig_idx];
                            let trapframe = task.get_trapframe();
                            setup_signal_frame(task, trapframe, self, sig, handler)?;
                            Ok(())
                        } else {
                            match sig {
                                SIGINT | SIGQUIT | SIGABRT | SIGSEGV | SIGTERM => {
                                    task.exit(128 + sig as i32);
                                    Ok(())
                                }
                                _ => Ok(()),
                            }
                        }
                    }
                    None => Ok(()),
                }
            }
            EventContent::Notification(ntype) => {
                match ntype {
                    crate::ipc::event::NotificationType::TaskCompleted => {
                        if let Some(parent_id) = task.get_parent_id() {
                            crate::task::wake_task_waiters(task.get_id());
                            crate::task::wake_parent_waiters(parent_id);
                        }
                    }
                    _ => {}
                }
                Ok(())
            }
            EventContent::Custom {
                namespace,
                event_id,
            } if namespace == "darwin.mach" => {
                if let crate::ipc::event::EventPayload::Bytes(bytes) = &event.payload {
                    self.store_mach_message(*event_id, bytes.clone());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn initialize_from_existing_handles(
        &mut self,
        _task: &crate::task::Task,
    ) -> Result<(), &'static str> {
        for fd in 0..3 {
            if let Some(pos) = self.free_fds.iter().position(|&x| x == fd) {
                self.free_fds.remove(pos);
            }
            self.fd_to_handle[fd] = Some(fd as u32);
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

fn create_dir_if_not_exists(vfs: &Arc<VfsManager>, path: &str) -> Result<(), FileSystemError> {
    match vfs.create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if e.kind == FileSystemErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

fn register_darwin_abi() {
    register_abi!(DarwinAarch64Abi);
}

late_initcall!(register_darwin_abi);
