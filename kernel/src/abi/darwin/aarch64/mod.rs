//! Darwin ABI module for AArch64

mod bsd_syscalls;
mod mach_syscalls;
mod syscall_table;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use hashbrown::HashMap;

use crate::{
    abi::AbiModule, arch::Trapframe, fs::SeekFrom, late_initcall, register_abi, task::mytask,
};

const MAX_FDS: usize = 1024;
const MACH_PORT_NULL: u32 = 0;

#[derive(Clone)]
pub struct DarwinAarch64Abi {
    fd_to_handle: Vec<Option<u32>>,
    free_fds: Vec<usize>,
    mach_task_port: u32,
    next_mach_port: u32,
    mach_ports: HashMap<u32, MachPortInfo>,
}

#[derive(Clone, Debug)]
struct MachPortInfo {
    right: MachPortRight,
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
            mach_task_port: 1,
            next_mach_port: 2,
            mach_ports: HashMap::new(),
        }
    }
}

impl DarwinAarch64Abi {
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
            SYS_getgid => Ok(bsd_syscalls::sys_getgid(self, trapframe)),
            SYS_socket => Ok(bsd_syscalls::sys_socket(self, trapframe)),
            SYS_dup => Ok(bsd_syscalls::sys_dup(self, trapframe)),
            SYS_dup2 => Ok(bsd_syscalls::sys_dup2(self, trapframe)),
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

    fn dispatch_mach_syscall(
        &mut self,
        num: i32,
        trapframe: &mut Trapframe,
    ) -> Result<usize, &'static str> {
        use syscall_table::*;
        match num {
            MACH_mach_reply_port | MACH_mach_port_allocate | MACH_task_for_pid => {
                if num == MACH_task_for_pid {
                    Ok(mach_syscalls::sys_task_for_pid(self, trapframe))
                } else if num == MACH_mach_port_allocate {
                    Ok(mach_syscalls::sys_mach_port_allocate(self, trapframe))
                } else {
                    let task = mytask().unwrap();
                    trapframe.increment_pc_next(task);
                    let port = self.next_mach_port;
                    self.next_mach_port += 1;
                    trapframe.set_return_value(port as usize);
                    Ok(port as usize)
                }
            }
            MACH_mach_port_deallocate => {
                Ok(mach_syscalls::sys_mach_port_deallocate(self, trapframe))
            }
            MACH_mach_msg_trap => Ok(mach_syscalls::sys_mach_msg_trap(self, trapframe)),
            MACH_vm_allocate => Ok(mach_syscalls::sys_vm_allocate(self, trapframe)),
            MACH_vm_deallocate => Ok(mach_syscalls::sys_vm_deallocate(self, trapframe)),
            MACH_mach_vm_allocate => Ok(mach_syscalls::sys_vm_allocate(self, trapframe)),
            MACH_mach_vm_deallocate => Ok(mach_syscalls::sys_vm_deallocate(self, trapframe)),
            MACH_thread_create | MACH_thread_create_running => {
                Ok(mach_syscalls::sys_thread_create(self, trapframe))
            }
            MACH_mach_timebase_info_trap => {
                Ok(mach_syscalls::sys_mach_timebase_info(self, trapframe))
            }
            MACH_clock_get_time => Ok(mach_syscalls::sys_clock_get_time(self, trapframe)),
            MACH_host_page_size => Ok(mach_syscalls::sys_host_page_size(self, trapframe)),
            _ => {
                crate::println!("[darwin] Unimplemented Mach trap: {}", num);
                let task = mytask().unwrap();
                trapframe.increment_pc_next(task);
                trapframe.set_return_value(super::error::KERN_FAILURE as usize);
                Ok(usize::MAX)
            }
        }
    }
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
            0x80 => {
                let bsd_num = syscall_num & 0xFFFFFF;
                self.dispatch_bsd_syscall(bsd_num, trapframe)
            }
            0x81 => {
                let mach_num = syscall_num as i32;
                self.dispatch_mach_syscall(mach_num, trapframe)
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
                        if magic_buffer == [0xFE, 0xED, 0xFA, 0xCF] {
                            40
                        } else if magic_buffer == [0xFE, 0xED, 0xFA, 0xCE] {
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
        _argv: &[&str],
        _envp: &[&str],
        _task: &crate::task::Task,
        _trapframe: &mut Trapframe,
    ) -> Result<(), &'static str> {
        crate::println!("[darwin] Mach-O loader not yet implemented");
        Err("Mach-O loader not implemented")
    }

    fn get_default_cwd(&self) -> &str {
        "/"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

fn register_darwin_abi() {
    register_abi!(DarwinAarch64Abi);
}

late_initcall!(register_darwin_abi);
