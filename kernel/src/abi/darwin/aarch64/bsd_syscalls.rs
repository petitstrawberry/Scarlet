use alloc::string::String;

use crate::abi::darwin::aarch64::DarwinAarch64Abi;
use crate::abi::darwin::aarch64::{
    SIGNAL_SAVED_PC_OFFSET, SIGNAL_SAVED_PSTATE_OFFSET, SIGNAL_SAVED_REGS_OFFSET,
    SIGNAL_SAVED_SP_OFFSET,
};
use crate::abi::darwin::error::*;
use crate::abi::darwin::path;
use crate::arch::Trapframe;
use crate::network::{NetworkManager, SocketDomain, SocketProtocol, SocketType};
use crate::object::KernelObject;
use crate::task::mytask;

const MAX_FDS: usize = 1024;

// Darwin socket constants
const AF_UNIX: i32 = 1;
const AF_INET: i32 = 2;
const AF_INET6: i32 = 30; // Darwin uses 30 for AF_INET6

const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const SOCK_TYPE_MASK: i32 = 0xF;

const DARWIN_O_RDONLY: i32 = 0x0;
const DARWIN_O_WRONLY: i32 = 0x1;
const DARWIN_O_RDWR: i32 = 0x2;
const DARWIN_O_CREAT: i32 = 0x200;
const DARWIN_O_TRUNC: i32 = 0x400;
const DARWIN_O_APPEND: i32 = 0x8;
const DARWIN_O_NONBLOCK: i32 = 0x4;
const DARWIN_O_DIRECTORY: i32 = 0x100000;

const DARWIN_PROT_NONE: i32 = 0;
const DARWIN_PROT_READ: i32 = 1;
const DARWIN_PROT_WRITE: i32 = 2;
const DARWIN_PROT_EXEC: i32 = 4;

const DARWIN_MAP_SHARED: i32 = 0x1;
const DARWIN_MAP_PRIVATE: i32 = 0x2;
const DARWIN_MAP_FIXED: i32 = 0x10;
const DARWIN_MAP_ANON: i32 = 0x1000;
const DARWIN_MAP_NOCACHE: i32 = 0x4000000;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DarwinStat {
    pub st_dev: u32,
    pub st_mode: u16,
    pub st_nlink: u16,
    pub st_ino: u64,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u32,
    pub st_atimespec: DarwinTimespec,
    pub st_mtimespec: DarwinTimespec,
    pub st_ctimespec: DarwinTimespec,
    pub st_birthtimespec: DarwinTimespec,
    pub st_size: i64,
    pub st_blocks: i64,
    pub st_blksize: i32,
    pub st_flags: u32,
    pub st_gen: u32,
    pub st_lspare: i32,
    pub st_qspare: [i64; 2],
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct DarwinTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

pub fn sys_exit(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.lock().store(trapframe);
    let status = trapframe.get_arg(0) as i32;
    task.exit(status);
    0
}

pub fn sys_fork(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let parent_task = mytask().unwrap();
    trapframe.increment_pc_next(parent_task);
    parent_task.vcpu.lock().store(trapframe);

    match parent_task.clone_task(crate::task::CloneFlags::default()) {
        Ok(mut child_task) => {
            let child_id = child_task.get_id();
            child_task.vcpu.lock().iregs.reg[0] = 0;
            crate::sched::scheduler::get_scheduler()
                .add_task(child_task, crate::arch::get_cpu().get_cpuid());
            trapframe.set_return_value(child_id);
            child_id
        }
        Err(_) => {
            // Darwin: set carry flag for error
            trapframe.spsr |= 1 << 29; // C=1 → error
            trapframe.set_return_value(ENOMEM);
            usize::MAX
        }
    }
}

pub fn sys_read(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let buf = trapframe.get_arg(1);
    let count = trapframe.get_arg(2);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let ko = match task.handle_table.get(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let file = match ko.as_file() {
        Some(f) => f,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    let mut buffer = alloc::vec![0u8; count];
    match file.read(&mut buffer) {
        Ok(n) => {
            if n > 0 && buf != 0 {
                write_user_bytes(task, buf, &buffer[..n]);
            }
            trapframe.set_return_value(n);
            n
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EIO);
            usize::MAX
        }
    }
}

pub fn sys_write(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let buf = trapframe.get_arg(1);
    let count = trapframe.get_arg(2);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let ko = match task.handle_table.get(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let file = match ko.as_file() {
        Some(f) => f,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    let data = read_user_bytes(task, buf, count);
    match file.write(&data) {
        Ok(n) => {
            trapframe.set_return_value(n);
            n
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EIO);
            usize::MAX
        }
    }
}

pub fn sys_open(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let path_ptr = trapframe.get_arg(0);
    let flags = trapframe.get_arg(1) as i32;
    let _mode = trapframe.get_arg(2) as u16;

    let darwin_path = match read_user_cstring(task, path_ptr) {
        Ok(p) => p,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EFAULT);
            return usize::MAX;
        }
    };

    let scarlet_path = path::translate_to_scarlet(&darwin_path);

    let vfs = match task.get_vfs() {
        Some(v) => v,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOENT);
            return usize::MAX;
        }
    };

    let open_flags = convert_open_flags(flags);
    let ko = match vfs.open(&scarlet_path, open_flags) {
        Ok(obj) => obj,
        Err(e) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(from_kernel_error(&e.message));
            return usize::MAX;
        }
    };

    let handle = match task.handle_table.insert(ko) {
        Ok(h) => h,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EMFILE);
            return usize::MAX;
        }
    };

    match abi.allocate_fd(handle) {
        Ok(fd) => {
            trapframe.set_return_value(fd);
            fd
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EMFILE);
            usize::MAX
        }
    }
}

pub fn sys_close(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;

    let handle = match abi.remove_fd(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let _ = task.handle_table.remove(handle);
    trapframe.set_return_value(0);
    0
}

pub fn sys_getpid(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    let pid = task.get_namespace_id();
    trapframe.set_return_value(pid);
    pid
}

pub fn sys_getppid(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    let ppid = task.get_parent_id().unwrap_or(0);
    trapframe.set_return_value(ppid);
    ppid
}

pub fn sys_getuid(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    trapframe.set_return_value(0);
    0
}

pub fn sys_getgid(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    trapframe.set_return_value(0);
    0
}

pub fn sys_socket(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let domain = trapframe.get_arg(0) as i32;
    let socket_type = trapframe.get_arg(1) as i32;
    let _protocol = trapframe.get_arg(2) as i32;
    let base_type = socket_type & SOCK_TYPE_MASK;

    let scarlet_domain = match domain {
        AF_UNIX => SocketDomain::Local,
        AF_INET => SocketDomain::Inet4,
        AF_INET6 => SocketDomain::Inet6,
        _ => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EAFNOSUPPORT);
            return usize::MAX;
        }
    };

    let scarlet_type = match base_type {
        SOCK_STREAM => SocketType::Stream,
        SOCK_DGRAM => SocketType::Datagram,
        SOCK_RAW => SocketType::Raw,
        _ => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ESOCKTNOSUPPORT);
            return usize::MAX;
        }
    };

    let mgr = NetworkManager::get_manager();
    let ko = match mgr.create_socket(scarlet_domain, scarlet_type, SocketProtocol::Default) {
        Ok(ko) => ko,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOBUFS);
            return usize::MAX;
        }
    };

    let handle = match task.handle_table.insert(ko) {
        Ok(h) => h,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EMFILE);
            return usize::MAX;
        }
    };

    match abi.allocate_fd(handle) {
        Ok(fd) => {
            trapframe.set_return_value(fd);
            fd
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EMFILE);
            usize::MAX
        }
    }
}

pub fn sys_dup(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let new_ko = match task.handle_table.clone_for_dup(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let new_handle = match task.handle_table.insert(new_ko) {
        Ok(h) => h,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EMFILE);
            return usize::MAX;
        }
    };

    match abi.allocate_fd(new_handle) {
        Ok(new_fd) => {
            trapframe.set_return_value(new_fd);
            new_fd
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EMFILE);
            usize::MAX
        }
    }
}

pub fn sys_dup2(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let old_fd = trapframe.get_arg(0) as usize;
    let new_fd = trapframe.get_arg(1) as usize;

    let old_handle = match abi.get_handle(old_fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    if let Some(existing) = abi.remove_fd(new_fd) {
        let _ = task.handle_table.remove(existing);
    }

    let new_ko = match task.handle_table.clone_for_dup(old_handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let new_handle = match task.handle_table.insert(new_ko) {
        Ok(h) => h,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };
    match abi.allocate_specific_fd(new_fd, new_handle) {
        Ok(()) => {
            trapframe.set_return_value(new_fd);
            new_fd
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            usize::MAX
        }
    }
}

pub fn sys_wait4(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    use crate::task::{WaitError, get_parent_waitpid_waker, get_waitpid_waker};

    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let pid = trapframe.get_arg(0) as i32;
    let status_ptr = trapframe.get_arg(1);
    let options = trapframe.get_arg(2) as i32;
    let _rusage_ptr = trapframe.get_arg(3);

    let wnohang = (options & 0x1) != 0;

    let return_error = |trapframe: &mut Trapframe, errno: usize| {
        trapframe.spsr |= 1 << 29;
        trapframe.set_return_value(errno);
        usize::MAX
    };

    let write_status = |status: i32, trapframe: &mut Trapframe| -> Result<(), usize> {
        if status_ptr == 0 {
            return Ok(());
        }

        let status_bytes = status.to_le_bytes();
        let mut checked = 0;
        while checked < status_bytes.len() {
            let current = status_ptr + checked;
            let page_off = current & (crate::environment::PAGE_SIZE - 1);
            let chunk = core::cmp::min(
                status_bytes.len() - checked,
                crate::environment::PAGE_SIZE - page_off,
            );

            if task.vm_manager.translate_to_kva(current).is_none() {
                return Err(return_error(trapframe, EFAULT));
            }

            checked += chunk;
        }

        write_user_bytes(task, status_ptr, &status_bytes);
        Ok(())
    };

    if task.get_children().is_empty() {
        return return_error(trapframe, ECHILD);
    }

    loop {
        if pid == -1 {
            for child_pid in task.get_children() {
                match task.wait(child_pid) {
                    Ok(status) => {
                        if let Err(err) = write_status(status, trapframe) {
                            return err;
                        }
                        trapframe.spsr &= !(1 << 29);
                        trapframe.set_return_value(child_pid);
                        return child_pid;
                    }
                    Err(WaitError::NoSuchChild(_)) | Err(WaitError::ChildTaskNotFound(_)) => {
                        continue;
                    }
                    Err(WaitError::ChildNotExited(_)) => {
                        continue;
                    }
                }
            }

            if wnohang {
                trapframe.spsr &= !(1 << 29);
                trapframe.set_return_value(0);
                return 0;
            }

            let parent_waker = get_parent_waitpid_waker(task.get_id());
            parent_waker.wait(task.get_id(), task.get_trapframe());
            continue;
        }

        if pid > 0 {
            let child_pid = pid as usize;

            if !task.get_children().contains(&child_pid) {
                return return_error(trapframe, ECHILD);
            }

            match task.wait(child_pid) {
                Ok(status) => {
                    if let Err(err) = write_status(status, trapframe) {
                        return err;
                    }
                    trapframe.spsr &= !(1 << 29);
                    trapframe.set_return_value(child_pid);
                    return child_pid;
                }
                Err(WaitError::NoSuchChild(_)) | Err(WaitError::ChildTaskNotFound(_)) => {
                    return return_error(trapframe, ECHILD);
                }
                Err(WaitError::ChildNotExited(_)) => {
                    if wnohang {
                        trapframe.spsr &= !(1 << 29);
                        trapframe.set_return_value(0);
                        return 0;
                    }

                    let child_waker = get_waitpid_waker(child_pid);
                    child_waker.wait(task.get_id(), task.get_trapframe());
                    continue;
                }
            }
        }

        return return_error(trapframe, EINVAL);
    }
}

pub fn sys_sigaction(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let sig = trapframe.get_arg(0) as usize;
    let act_ptr = trapframe.get_arg(1);
    let oact_ptr = trapframe.get_arg(2);

    if sig == 0 || sig >= 32 {
        trapframe.spsr |= 1 << 29;
        trapframe.set_return_value(EINVAL);
        return usize::MAX;
    }

    if oact_ptr != 0 {
        let old_handler = abi.signal_handlers[sig];
        let old_act_bytes = old_handler.to_le_bytes();
        write_user_bytes(task, oact_ptr, &old_act_bytes);
    }

    if act_ptr != 0 {
        let handler_bytes = read_user_bytes(task, act_ptr, core::mem::size_of::<usize>());
        if handler_bytes.len() >= core::mem::size_of::<usize>() {
            let handler = usize::from_le_bytes([
                handler_bytes[0],
                handler_bytes[1],
                handler_bytes[2],
                handler_bytes[3],
                handler_bytes[4],
                handler_bytes[5],
                handler_bytes[6],
                handler_bytes[7],
            ]);
            abi.signal_handlers[sig] = handler;
        }
    }

    trapframe.set_return_value(0);
    0
}

pub fn sys_sigreturn(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    let frame_base = trapframe.get_arg(0);
    if frame_base == 0 {
        trapframe.spsr |= 1 << 29;
        trapframe.set_return_value(EINVAL);
        return usize::MAX;
    }

    unsafe {
        for i in 0..31 {
            let kva = match task
                .vm_manager
                .translate_to_kva(frame_base + SIGNAL_SAVED_REGS_OFFSET + i * 8)
            {
                Some(kva) => kva,
                None => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EFAULT);
                    return usize::MAX;
                }
            };
            trapframe.regs.reg[i] = *(kva as *const usize);
        }

        let saved_sp = match task
            .vm_manager
            .translate_to_kva(frame_base + SIGNAL_SAVED_SP_OFFSET)
        {
            Some(kva) => kva,
            None => {
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(EFAULT);
                return usize::MAX;
            }
        };
        trapframe.sp = *(saved_sp as *const u64);

        let saved_pc = match task
            .vm_manager
            .translate_to_kva(frame_base + SIGNAL_SAVED_PC_OFFSET)
        {
            Some(kva) => kva,
            None => {
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(EFAULT);
                return usize::MAX;
            }
        };
        trapframe.elr = *(saved_pc as *const u64);

        let saved_pstate = match task
            .vm_manager
            .translate_to_kva(frame_base + SIGNAL_SAVED_PSTATE_OFFSET)
        {
            Some(kva) => kva,
            None => {
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(EFAULT);
                return usize::MAX;
            }
        };
        trapframe.spsr = *(saved_pstate as *const u64);
    }

    trapframe.spsr &= !(1 << 29);
    trapframe.set_return_value(0);
    0
}

pub fn sys_fcntl(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let cmd = trapframe.get_arg(1) as i32;
    let _arg = trapframe.get_arg(2);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    match cmd {
        0 => {
            let new_ko = match task.handle_table.clone_for_dup(handle) {
                Some(ko) => ko,
                None => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EBADF);
                    return usize::MAX;
                }
            };
            let new_handle = match task.handle_table.insert(new_ko) {
                Ok(h) => h,
                Err(_) => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EMFILE);
                    return usize::MAX;
                }
            };
            match abi.allocate_fd(new_handle) {
                Ok(new_fd) => {
                    trapframe.set_return_value(new_fd);
                    new_fd
                }
                Err(_) => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EMFILE);
                    usize::MAX
                }
            }
        }
        1 => {
            trapframe.set_return_value(0);
            0
        }
        2 => {
            trapframe.set_return_value(0);
            0
        }
        3 => {
            trapframe.set_return_value(0);
            0
        }
        4 => {
            trapframe.set_return_value(0);
            0
        }
        _ => {
            crate::println!("[darwin] Unimplemented fcntl cmd: {}", cmd);
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            usize::MAX
        }
    }
}

pub fn sys_ioctl(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let request = trapframe.get_arg(1);
    let _arg = trapframe.get_arg(2);

    let _handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    crate::println!("[darwin] ioctl stub: fd={}, request={:#x}", fd, request);
    trapframe.set_return_value(0);
    0
}

pub fn sys_lseek(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let ko = match task.handle_table.get(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let file = match ko.as_file() {
        Some(f) => f,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    let seek_from = match whence {
        0 => crate::fs::SeekFrom::Start(offset as u64),
        1 => crate::fs::SeekFrom::Current(offset),
        2 => crate::fs::SeekFrom::End(offset),
        _ => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    match file.seek(seek_from) {
        Ok(pos) => {
            trapframe.set_return_value(pos as usize);
            pos as usize
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            usize::MAX
        }
    }
}

// Helper functions

fn convert_open_flags(darwin_flags: i32) -> u32 {
    let mut flags: u32 = 0;

    match darwin_flags & 0x3 {
        DARWIN_O_RDONLY => {}
        DARWIN_O_WRONLY => flags |= 0x1,
        DARWIN_O_RDWR => flags |= 0x2,
        _ => {}
    }

    if darwin_flags & DARWIN_O_CREAT != 0 {
        flags |= 0x40;
    }
    if darwin_flags & DARWIN_O_TRUNC != 0 {
        flags |= 0x200;
    }
    if darwin_flags & DARWIN_O_APPEND != 0 {
        flags |= 0x400;
    }

    flags
}

fn read_user_bytes(task: &crate::task::Task, vaddr: usize, len: usize) -> alloc::vec::Vec<u8> {
    let mut buf = alloc::vec![0u8; len];
    let mut written = 0;
    while written < len {
        let current = vaddr + written;
        let page_off = current & (crate::environment::PAGE_SIZE - 1);
        let chunk = core::cmp::min(len - written, crate::environment::PAGE_SIZE - page_off);
        if let Some(kaddr) = task.vm_manager.translate_to_kva(current) {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    kaddr as *const u8,
                    buf.as_mut_ptr().add(written),
                    chunk,
                );
            }
        } else {
            break;
        }
        written += chunk;
    }
    buf.truncate(written);
    buf
}

fn write_user_bytes(task: &crate::task::Task, vaddr: usize, data: &[u8]) {
    let mut written = 0;
    while written < data.len() {
        let current = vaddr + written;
        let page_off = current & (crate::environment::PAGE_SIZE - 1);
        let chunk = core::cmp::min(
            data.len() - written,
            crate::environment::PAGE_SIZE - page_off,
        );
        if let Some(kaddr) = task.vm_manager.translate_to_kva(current) {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[written..written + chunk].as_ptr(),
                    kaddr as *mut u8,
                    chunk,
                );
            }
        } else {
            break;
        }
        written += chunk;
    }
}

fn read_user_cstring(task: &crate::task::Task, vaddr: usize) -> Result<String, ()> {
    let mut bytes = alloc::vec::Vec::new();
    let max_len = 4096;
    let mut current = vaddr;
    for _ in 0..max_len {
        match task.vm_manager.translate_to_kva(current) {
            Some(kaddr) => {
                let byte = unsafe { *(kaddr as *const u8) };
                if byte == 0 {
                    break;
                }
                bytes.push(byte);
                current += 1;
            }
            None => return Err(()),
        }
    }
    String::from_utf8(bytes).map_err(|_| ())
}

pub fn sys_bind(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let _addr_ptr = trapframe.get_arg(1);
    let _addrlen = trapframe.get_arg(2);

    let _handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    crate::println!("[darwin] bind stub: fd={}", fd);
    trapframe.set_return_value(0);
    0
}

pub fn sys_connect(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let _addr_ptr = trapframe.get_arg(1);
    let _addrlen = trapframe.get_arg(2);

    let _handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    crate::println!("[darwin] connect stub: fd={}", fd);
    trapframe.set_return_value(0);
    0
}

pub fn sys_listen(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let _backlog = trapframe.get_arg(1);

    let _handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    crate::println!("[darwin] listen stub: fd={}", fd);
    trapframe.set_return_value(0);
    0
}

pub fn sys_accept(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let _addr_ptr = trapframe.get_arg(1);
    let _addrlen_ptr = trapframe.get_arg(2);

    let _handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    crate::println!("[darwin] accept stub: fd={}", fd);
    trapframe.spsr |= 1 << 29;
    trapframe.set_return_value(EAGAIN);
    usize::MAX
}

pub fn sys_sendto(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = trapframe.get_arg(1);
    let len = trapframe.get_arg(2);
    let _flags = trapframe.get_arg(3);
    let _addr_ptr = trapframe.get_arg(4);
    let _addrlen = trapframe.get_arg(5);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let ko = match task.handle_table.get(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let file = match ko.as_file() {
        Some(f) => f,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    let data = read_user_bytes(task, buf_ptr, len);
    match file.write(&data) {
        Ok(n) => {
            trapframe.set_return_value(n);
            n
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EPIPE);
            usize::MAX
        }
    }
}

pub fn sys_recvfrom(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = trapframe.get_arg(1);
    let len = trapframe.get_arg(2);
    let _flags = trapframe.get_arg(3);
    let _addr_ptr = trapframe.get_arg(4);
    let _addrlen_ptr = trapframe.get_arg(5);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let ko = match task.handle_table.get(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let file = match ko.as_file() {
        Some(f) => f,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    let mut buffer = alloc::vec![0u8; len];
    match file.read(&mut buffer) {
        Ok(n) => {
            if n > 0 && buf_ptr != 0 {
                write_user_bytes(task, buf_ptr, &buffer[..n]);
            }
            trapframe.set_return_value(n);
            n
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EAGAIN);
            usize::MAX
        }
    }
}

pub fn sys_shutdown(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let _how = trapframe.get_arg(1);

    let _handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    crate::println!("[darwin] shutdown stub: fd={}", fd);
    trapframe.set_return_value(0);
    0
}

fn write_user_struct<T: Copy>(task: &crate::task::Task, vaddr: usize, data: &T) {
    let size = core::mem::size_of::<T>();
    let bytes = unsafe { core::slice::from_raw_parts(data as *const T as *const u8, size) };
    write_user_bytes(task, vaddr, bytes);
}

pub fn sys_mmap(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let addr = trapframe.get_arg(0);
    let len = trapframe.get_arg(1);
    let prot = trapframe.get_arg(2) as i32;
    let flags = trapframe.get_arg(3) as i32;
    let fd = trapframe.get_arg(4) as i32;
    let offset = trapframe.get_arg(5);

    if len == 0 {
        trapframe.spsr |= 1 << 29;
        trapframe.set_return_value(EINVAL);
        return usize::MAX;
    }

    let mut scarlet_prot = 0usize;
    if prot & DARWIN_PROT_READ != 0 {
        scarlet_prot |= 0x01;
    }
    if prot & DARWIN_PROT_WRITE != 0 {
        scarlet_prot |= 0x02;
    }
    if prot & DARWIN_PROT_EXEC != 0 {
        scarlet_prot |= 0x04;
    }
    scarlet_prot |= 0x08;

    let aligned_len =
        (len + crate::environment::PAGE_SIZE - 1) & !(crate::environment::PAGE_SIZE - 1);

    let vaddr = if flags & DARWIN_MAP_FIXED != 0 {
        addr
    } else {
        match task
            .vm_manager
            .find_unmapped_area(aligned_len, crate::environment::PAGE_SIZE)
        {
            Some(a) => a,
            None => {
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(ENOMEM);
                return usize::MAX;
            }
        }
    };

    if flags & DARWIN_MAP_ANON != 0 {
        let num_pages = aligned_len / crate::environment::PAGE_SIZE;
        let pages = match crate::mem::page::ContiguousPages::new(num_pages) {
            Some(p) => p,
            None => {
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(ENOMEM);
                return usize::MAX;
            }
        };
        let paddr = pages.as_paddr();
        let mmap = crate::vm::vmem::VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea::new(paddr, paddr + aligned_len - 1),
            crate::vm::vmem::MemoryArea::new(vaddr, vaddr + aligned_len - 1),
            scarlet_prot,
            flags & DARWIN_MAP_SHARED != 0,
            None,
        );
        match task.vm_manager.add_memory_map(mmap) {
            Ok(()) => {
                task.page_allocations.write().push(pages);
                if let Some(kva) = task.vm_manager.translate_to_kva(vaddr) {
                    unsafe {
                        core::ptr::write_bytes(kva as *mut u8, 0, aligned_len);
                    }
                }
                trapframe.set_return_value(vaddr);
                vaddr
            }
            Err(_) => {
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(ENOMEM);
                usize::MAX
            }
        }
    } else {
        if fd == -1 {
            let num_pages = aligned_len / crate::environment::PAGE_SIZE;
            let pages = match crate::mem::page::ContiguousPages::new(num_pages) {
                Some(p) => p,
                None => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(ENOMEM);
                    return usize::MAX;
                }
            };
            let paddr = pages.as_paddr();
            let mmap = crate::vm::vmem::VirtualMemoryMap::new(
                crate::vm::vmem::MemoryArea::new(paddr, paddr + aligned_len - 1),
                crate::vm::vmem::MemoryArea::new(vaddr, vaddr + aligned_len - 1),
                scarlet_prot,
                flags & DARWIN_MAP_SHARED != 0,
                None,
            );
            match task.vm_manager.add_memory_map(mmap) {
                Ok(()) => {
                    task.page_allocations.write().push(pages);
                    trapframe.set_return_value(vaddr);
                    vaddr
                }
                Err(_) => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(ENOMEM);
                    usize::MAX
                }
            }
        } else {
            use crate::object::capability::memory_mapping::MemoryMappingOps;
            use crate::vm::addr::{is_direct_mapped, virt_to_phys};

            let handle = match abi.get_handle(fd as usize) {
                Some(h) => h,
                None => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EBADF);
                    return usize::MAX;
                }
            };

            let kernel_obj = match task.handle_table.get(handle) {
                Some(obj) => obj,
                None => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EBADF);
                    return usize::MAX;
                }
            };
            let file_obj = kernel_obj.as_file();

            let memory_mappable: &dyn MemoryMappingOps = match kernel_obj.as_memory_mappable() {
                Some(mappable) => mappable,
                None => {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(ENODEV);
                    return usize::MAX;
                }
            };

            if !memory_mappable.supports_mmap() {
                trapframe.spsr |= 1 << 29;
                trapframe.set_return_value(ENODEV);
                return usize::MAX;
            }

            let is_shared = flags & DARWIN_MAP_SHARED != 0;
            let mut ok_len = aligned_len;
            let (mapping_base, obj_permissions, _obj_is_shared) = loop {
                match memory_mappable.get_mapping_info_with(offset, ok_len, is_shared) {
                    Ok(info) => break info,
                    Err(_) => {
                        if ok_len >= crate::environment::PAGE_SIZE {
                            ok_len -= crate::environment::PAGE_SIZE;
                        } else {
                            ok_len = 0;
                        }
                        if ok_len == 0 {
                            break (0, 0, false);
                        }
                    }
                }
            };

            let paddr = if mapping_base != 0 && is_direct_mapped(mapping_base) {
                virt_to_phys(mapping_base)
            } else {
                mapping_base
            };

            let mut prot_mask = 0usize;
            if prot & DARWIN_PROT_READ != 0 {
                prot_mask |= 0x01;
            }
            if prot & DARWIN_PROT_WRITE != 0 {
                prot_mask |= 0x02;
            }
            if prot & DARWIN_PROT_EXEC != 0 {
                prot_mask |= 0x04;
            }

            let is_private = flags & DARWIN_MAP_PRIVATE != 0;
            let mut final_permissions = if is_private {
                prot_mask
            } else {
                obj_permissions & prot_mask
            };
            if prot != DARWIN_PROT_NONE {
                final_permissions |= 0x08;
            }

            if is_private && !is_shared {
                const PAGES_PER_ALLOC: usize = 16;

                let num_pages = aligned_len / crate::environment::PAGE_SIZE;
                let num_allocs = num_pages.div_ceil(PAGES_PER_ALLOC);
                let mut page_allocs: alloc::vec::Vec<crate::mem::page::TaskPages> =
                    alloc::vec::Vec::with_capacity(num_allocs);
                let mut vm_maps: alloc::vec::Vec<crate::vm::vmem::VirtualMemoryMap> =
                    alloc::vec::Vec::with_capacity(num_pages);

                for alloc_idx in 0..num_allocs {
                    let pages_in_this_alloc = if alloc_idx == num_allocs - 1 {
                        num_pages - alloc_idx * PAGES_PER_ALLOC
                    } else {
                        PAGES_PER_ALLOC
                    };

                    let alloc = match crate::mem::page::TaskPages::new(pages_in_this_alloc) {
                        Some(a) => a,
                        None => {
                            trapframe.spsr |= 1 << 29;
                            trapframe.set_return_value(ENOMEM);
                            return usize::MAX;
                        }
                    };

                    let chunk_vaddr =
                        vaddr + alloc_idx * PAGES_PER_ALLOC * crate::environment::PAGE_SIZE;
                    page_allocs.push(alloc);

                    for page_idx_in_alloc in 0..pages_in_this_alloc {
                        let page_vaddr =
                            chunk_vaddr + page_idx_in_alloc * crate::environment::PAGE_SIZE;
                        let page_vmarea = crate::vm::vmem::MemoryArea::new(
                            page_vaddr,
                            page_vaddr + crate::environment::PAGE_SIZE - 1,
                        );
                        let page_paddr = page_allocs[alloc_idx]
                            .page_paddr(page_idx_in_alloc)
                            .unwrap();
                        let page_pmarea = crate::vm::vmem::MemoryArea::new(
                            page_paddr,
                            page_paddr + crate::environment::PAGE_SIZE - 1,
                        );
                        vm_maps.push(crate::vm::vmem::VirtualMemoryMap::new(
                            page_pmarea,
                            page_vmarea,
                            final_permissions,
                            false,
                            None,
                        ));
                    }
                }

                let mut removed_mappings_opt: Option<
                    alloc::vec::Vec<crate::vm::vmem::VirtualMemoryMap>,
                > = None;

                for vm_map in &vm_maps {
                    let result = if flags & DARWIN_MAP_FIXED != 0 {
                        task.vm_manager
                            .add_memory_map_fixed(vm_map.clone())
                            .map(Some)
                    } else {
                        task.vm_manager.add_memory_map(vm_map.clone()).map(|_| None)
                    };

                    match result {
                        Ok(removed) => {
                            if let Some(rm) = removed {
                                if removed_mappings_opt.is_none() {
                                    removed_mappings_opt = Some(alloc::vec::Vec::new());
                                }
                                removed_mappings_opt.as_mut().unwrap().extend(rm);
                            }
                        }
                        Err(_) => {
                            trapframe.spsr |= 1 << 29;
                            trapframe.set_return_value(ENOMEM);
                            return usize::MAX;
                        }
                    }
                }

                if let Some(ref removed_mappings) = removed_mappings_opt {
                    for removed_map in removed_mappings {
                        if removed_map.is_shared {
                            if let Some(owner_weak) = &removed_map.owner {
                                if let Some(owner) = owner_weak.upgrade() {
                                    owner.on_unmapped(
                                        removed_map.vmarea.start,
                                        removed_map.vmarea.size(),
                                    );
                                }
        }
    }
}

pub fn sys_thread_selfid(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    let tid = task.get_id() as u64;
    crate::println!("[darwin] thread_selfid: tid={:#x}", tid);
    trapframe.set_return_value(tid as usize);
    tid as usize
}

pub fn sys_proc_info(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    trapframe.set_return_value(0);
    0
}

pub fn sys_getentropy(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let buf = trapframe.get_arg(0);
    let size = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);

    if size > 256 {
        trapframe.set_return_value(EINVAL);
        return EINVAL;
    }

    if let Some(kva) = task.vm_manager.translate_to_kva(buf) {
        let bytes = unsafe { core::slice::from_raw_parts_mut(kva as *mut u8, size) };
        for b in bytes.iter_mut() {
            *b = 0x42;
        }
        trapframe.set_return_value(0);
        0
    } else {
        trapframe.set_return_value(EFAULT);
        EFAULT
    }
}

pub fn sys_getlogin(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let namebuf = trapframe.get_arg(0);
    let _namelen = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);

    let login = b"root\0";
    if let Some(kva) = task.vm_manager.translate_to_kva(namebuf) {
        let dst = unsafe { core::slice::from_raw_parts_mut(kva as *mut u8, login.len()) };
        dst.copy_from_slice(login);
        trapframe.set_return_value(0);
        0
    } else {
        trapframe.set_return_value(EFAULT);
        EFAULT
    }
}
                 }

                if let Some(removed_mappings) = removed_mappings_opt {
                    for removed_map in removed_mappings {
                        crate::object::capability::memory_mapping::syscall::reclaim_private_removed_mapping(task, &removed_map);
                    }
                }

                let mut page_idx = 0;
                for alloc in &page_allocs {
                    for local_idx in 0..alloc.len() {
                        let dst_paddr = alloc.page_paddr(local_idx).unwrap();
                        let dst_vaddr = crate::vm::addr::phys_to_virt(dst_paddr);
                        unsafe {
                            core::ptr::write_bytes(
                                dst_vaddr as *mut u8,
                                0,
                                crate::environment::PAGE_SIZE,
                            );
                        }

                        if ok_len > 0 {
                            let copy_start = page_idx * crate::environment::PAGE_SIZE;
                            if copy_start < ok_len {
                                let to_copy = core::cmp::min(
                                    ok_len - copy_start,
                                    crate::environment::PAGE_SIZE,
                                );
                                if let Some(file_obj) = file_obj {
                                    let read_offset = match offset.checked_add(copy_start) {
                                        Some(read_offset) => read_offset as u64,
                                        None => {
                                            trapframe.spsr |= 1 << 29;
                                            trapframe.set_return_value(EINVAL);
                                            return usize::MAX;
                                        }
                                    };
                                    let dst_slice = unsafe {
                                        core::slice::from_raw_parts_mut(
                                            dst_vaddr as *mut u8,
                                            to_copy,
                                        )
                                    };
                                    if file_obj.read_at(read_offset, dst_slice).is_err() {
                                        trapframe.spsr |= 1 << 29;
                                        trapframe.set_return_value(EIO);
                                        return usize::MAX;
                                    }
                                } else if paddr != 0 {
                                    unsafe {
                                        core::ptr::copy_nonoverlapping(
                                            crate::vm::addr::phys_to_virt(paddr + copy_start)
                                                as *const u8,
                                            dst_vaddr as *mut u8,
                                            to_copy,
                                        );
                                    }
                                } else {
                                    trapframe.spsr |= 1 << 29;
                                    trapframe.set_return_value(EINVAL);
                                    return usize::MAX;
                                }
                            }
                        }

                        page_idx += 1;
                    }
                }

                task.task_pages.write().extend(page_allocs);
                trapframe.set_return_value(vaddr);
                vaddr
            } else {
                if paddr == 0 && ok_len == 0 {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EINVAL);
                    return usize::MAX;
                }

                let ok_len_aligned =
                    (ok_len / crate::environment::PAGE_SIZE) * crate::environment::PAGE_SIZE;
                if ok_len_aligned == 0 {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EINVAL);
                    return usize::MAX;
                }

                let vm_map = crate::vm::vmem::VirtualMemoryMap::new(
                    crate::vm::vmem::MemoryArea::new(paddr, paddr + ok_len_aligned - 1),
                    crate::vm::vmem::MemoryArea::new(vaddr, vaddr + ok_len_aligned - 1),
                    final_permissions,
                    is_shared,
                    kernel_obj.as_memory_mappable_weak(),
                );

                let map_result = if flags & DARWIN_MAP_FIXED != 0 {
                    task.vm_manager.add_memory_map_fixed(vm_map).map(Some)
                } else {
                    task.vm_manager.add_memory_map(vm_map).map(|_| None)
                };

                match map_result {
                    Ok(removed_mappings_opt) => {
                        memory_mappable.on_mapped(vaddr, paddr, ok_len_aligned, offset);

                        if let Some(removed_mappings) = &removed_mappings_opt {
                            for removed_map in removed_mappings {
                                if removed_map.is_shared {
                                    if let Some(owner_weak) = &removed_map.owner {
                                        if let Some(owner) = owner_weak.upgrade() {
                                            owner.on_unmapped(
                                                removed_map.vmarea.start,
                                                removed_map.vmarea.size(),
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(removed_mappings) = removed_mappings_opt {
                            for removed_map in removed_mappings {
                                crate::object::capability::memory_mapping::syscall::reclaim_private_removed_mapping(task, &removed_map);
                            }
                        }

                        trapframe.set_return_value(vaddr);
                        vaddr
                    }
                    Err(_) => {
                        trapframe.spsr |= 1 << 29;
                        trapframe.set_return_value(ENOMEM);
                        usize::MAX
                    }
                }
            }
        }
    }
}

pub fn sys_munmap(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let addr = trapframe.get_arg(0);
    let _len = trapframe.get_arg(1);

    let _ = task.vm_manager.remove_memory_map_by_addr(addr);
    trapframe.set_return_value(0);
    0
}

pub fn sys_mprotect(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _addr = trapframe.get_arg(0);
    let _len = trapframe.get_arg(1);
    let _prot = trapframe.get_arg(2) as i32;

    trapframe.set_return_value(0);
    0
}

pub fn sys_stat(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let path_ptr = trapframe.get_arg(0);
    let stat_ptr = trapframe.get_arg(1);

    let darwin_path = match read_user_cstring(task, path_ptr) {
        Ok(p) => p,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EFAULT);
            return usize::MAX;
        }
    };

    let scarlet_path = path::translate_to_scarlet(&darwin_path);
    do_stat(task, abi, &scarlet_path, stat_ptr, false, trapframe)
}

pub fn sys_lstat(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let path_ptr = trapframe.get_arg(0);
    let stat_ptr = trapframe.get_arg(1);

    let darwin_path = match read_user_cstring(task, path_ptr) {
        Ok(p) => p,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EFAULT);
            return usize::MAX;
        }
    };

    let scarlet_path = path::translate_to_scarlet(&darwin_path);
    do_stat(task, abi, &scarlet_path, stat_ptr, true, trapframe)
}

pub fn sys_fstat(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let stat_ptr = trapframe.get_arg(1);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let ko = match task.handle_table.get(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let file = match ko.as_file() {
        Some(f) => f,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    match file.metadata() {
        Ok(meta) => {
            let stat = DarwinStat {
                st_dev: 0,
                st_mode: 0o100644,
                st_nlink: 1,
                st_ino: 0,
                st_uid: 0,
                st_gid: 0,
                st_rdev: 0,
                st_atimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_mtimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_ctimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_birthtimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_size: meta.size as i64,
                st_blocks: ((meta.size + 511) / 512) as i64,
                st_blksize: 4096,
                st_flags: 0,
                st_gen: 0,
                st_lspare: 0,
                st_qspare: [0; 2],
            };
            write_user_struct(task, stat_ptr, &stat);
            trapframe.set_return_value(0);
            0
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EIO);
            usize::MAX
        }
    }
}

fn do_stat(
    task: &crate::task::Task,
    _abi: &mut DarwinAarch64Abi,
    path: &str,
    stat_ptr: usize,
    _follow_symlinks: bool,
    trapframe: &mut Trapframe,
) -> usize {
    let vfs = match task.get_vfs() {
        Some(v) => v,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOENT);
            return usize::MAX;
        }
    };

    let ko = match vfs.open(path, 0) {
        Ok(obj) => obj,
        Err(e) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(from_kernel_error(&e.message));
            return usize::MAX;
        }
    };

    let file = match ko.as_file() {
        Some(f) => f,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EINVAL);
            return usize::MAX;
        }
    };

    match file.metadata() {
        Ok(meta) => {
            let stat = DarwinStat {
                st_dev: 0,
                st_mode: 0o100644,
                st_nlink: 1,
                st_ino: 0,
                st_uid: 0,
                st_gid: 0,
                st_rdev: 0,
                st_atimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_mtimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_ctimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_birthtimespec: DarwinTimespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                st_size: meta.size as i64,
                st_blocks: ((meta.size + 511) / 512) as i64,
                st_blksize: 4096,
                st_flags: 0,
                st_gen: 0,
                st_lspare: 0,
                st_qspare: [0; 2],
            };
            write_user_struct(task, stat_ptr, &stat);
            trapframe.set_return_value(0);
            0
        }
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EIO);
            usize::MAX
        }
    }
}

/// Darwin dirent structure for getdirentries
#[repr(C)]
#[derive(Clone, Copy)]
struct DarwinDirent {
    d_ino: u64,
    d_seekoff: u64,
    d_reclen: u16,
    d_namlen: u16,
    d_type: u8,
    d_name: [u8; 0],
}

impl DarwinDirent {
    fn file_type(file_type: u8) -> u8 {
        match file_type {
            0 => 8,
            1 => 4,
            2 => 10,
            3 => 2,
            4 => 6,
            5 => 1,
            6 => 12,
            _ => 0,
        }
    }

    fn new(entry: &crate::fs::DirectoryEntry, d_seekoff: u64) -> Self {
        let name_len = entry.name_len as usize;
        let reclen = (core::mem::size_of::<Self>() + name_len + 1 + 7) & !7;
        Self {
            d_ino: entry.file_id,
            d_seekoff,
            d_reclen: reclen as u16,
            d_namlen: name_len as u16,
            d_type: Self::file_type(entry.file_type),
            d_name: [],
        }
    }

    fn to_bytes(entry: &crate::fs::DirectoryEntry, d_seekoff: u64) -> alloc::vec::Vec<u8> {
        let dirent = Self::new(entry, d_seekoff);
        let header_len = core::mem::size_of::<Self>();
        let name_len = entry.name_len as usize;
        let mut bytes = alloc::vec![0u8; dirent.d_reclen as usize];

        unsafe {
            core::ptr::copy_nonoverlapping(
                &dirent as *const Self as *const u8,
                bytes.as_mut_ptr(),
                header_len,
            );
        }
        bytes[header_len..header_len + name_len].copy_from_slice(&entry.name[..name_len]);
        bytes[header_len + name_len] = 0;
        bytes
    }
}

pub fn sys_getdirentries(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = trapframe.get_arg(1);
    let bufsize = trapframe.get_arg(2) as usize;
    let basep_ptr = trapframe.get_arg(3);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let ko = match task.handle_table.get(handle) {
        Some(ko) => ko,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EBADF);
            return usize::MAX;
        }
    };

    let stream = match ko.as_stream() {
        Some(s) => s,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOTDIR);
            return usize::MAX;
        }
    };

    let mut dir_buffer = alloc::vec![0u8; core::mem::size_of::<crate::fs::DirectoryEntry>()];
    let mut written = 0usize;
    let mut cookie = 0u64;

    loop {
        match stream.read(&mut dir_buffer) {
            Ok(n) if n == dir_buffer.len() => {
                let entry = match crate::fs::DirectoryEntry::parse(&dir_buffer) {
                    Some(entry) => entry,
                    None => {
                        if written == 0 {
                            trapframe.spsr |= 1 << 29;
                            trapframe.set_return_value(EIO);
                            return usize::MAX;
                        }
                        break;
                    }
                };

                let next_cookie = cookie + 1;
                let dirent_bytes = DarwinDirent::to_bytes(&entry, next_cookie);
                if written + dirent_bytes.len() > bufsize {
                    break;
                }

                if !dirent_bytes.is_empty() && buf_ptr != 0 {
                    write_user_bytes(task, buf_ptr + written, &dirent_bytes);
                }
                written += dirent_bytes.len();
                cookie = next_cookie;
            }
            Ok(0) => break,
            Ok(_) => {
                if written == 0 {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EIO);
                    return usize::MAX;
                }
                break;
            }
            Err(crate::object::capability::StreamError::EndOfStream) => break,
            Err(crate::object::capability::StreamError::WouldBlock) => {
                crate::sched::scheduler::get_scheduler().schedule(trapframe);
                return usize::MAX;
            }
            Err(_) => {
                if written == 0 {
                    trapframe.spsr |= 1 << 29;
                    trapframe.set_return_value(EIO);
                    return usize::MAX;
                }
                break;
            }
        }
    }

    if basep_ptr != 0 {
        let basep = (cookie as i64).to_ne_bytes();
        write_user_bytes(task, basep_ptr, &basep);
    }

    trapframe.set_return_value(written);
    written
}

fn resolve_path(task: &crate::task::Task, path: &str) -> Option<String> {
    let raw_path = if path.starts_with('/') {
        String::from(path)
    } else {
        let cwd = task
            .get_vfs()
            .map(|vfs| vfs.get_cwd_path())
            .unwrap_or_else(|| String::from("/"));
        if cwd == "/" {
            let mut full_path = String::from("/");
            full_path.push_str(path);
            full_path
        } else {
            let mut full_path = String::from(cwd.trim_end_matches('/'));
            full_path.push('/');
            full_path.push_str(path);
            full_path
        }
    };

    let mut components = alloc::vec::Vec::new();
    for component in raw_path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(component),
        }
    }

    let mut resolved = String::from("/");
    if !components.is_empty() {
        resolved.push_str(&components.join("/"));
    }

    Some(resolved)
}

fn parse_string_array(task: &crate::task::Task, ptr: usize) -> Result<alloc::vec::Vec<String>, ()> {
    if ptr == 0 {
        return Ok(alloc::vec::Vec::new());
    }

    let mut strings = alloc::vec::Vec::new();
    let word_size = core::mem::size_of::<usize>();

    for index in 0..256usize {
        let entry_addr = ptr + index * word_size;
        let entry_bytes = read_user_bytes(task, entry_addr, word_size);
        if entry_bytes.len() != word_size {
            return Err(());
        }

        let entry_ptr = usize::from_ne_bytes(entry_bytes.try_into().map_err(|_| ())?);
        if entry_ptr == 0 {
            return Ok(strings);
        }

        strings.push(read_user_cstring(task, entry_ptr)?);
    }

    Err(())
}

fn open_executable(task: &crate::task::Task, path: &str) -> Option<KernelObject> {
    let vfs = task.get_vfs()?;
    vfs.open(path, 0).ok()
}

pub fn sys_execve(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let path_ptr = trapframe.get_arg(0);
    let argv_ptr = trapframe.get_arg(1);
    let envp_ptr = trapframe.get_arg(2);

    let path_str = match read_user_cstring(task, path_ptr) {
        Ok(s) => s,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EFAULT);
            return usize::MAX;
        }
    };

    let abs_path = match resolve_path(task, &path_str) {
        Some(p) => p,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOENT);
            return usize::MAX;
        }
    };

    let argv_strings = match parse_string_array(task, argv_ptr) {
        Ok(v) => v,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(EFAULT);
            return usize::MAX;
        }
    };

    let envp_strings = match parse_string_array(task, envp_ptr) {
        Ok(v) => v,
        Err(_) => alloc::vec::Vec::new(),
    };

    let file_object = match open_executable(task, &abs_path) {
        Some(obj) => obj,
        None => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOENT);
            return usize::MAX;
        }
    };

    let argv_refs: alloc::vec::Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();
    let envp_refs: alloc::vec::Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();

    match crate::abi::AbiModule::execute_binary(
        abi,
        &file_object,
        &argv_refs,
        &envp_refs,
        task,
        trapframe,
    ) {
        Ok(()) => trapframe.get_return_value(),
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOEXEC);
            usize::MAX
        }
    }
}

pub fn sys_thread_selfid(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    let tid = task.get_id() as u64;
    crate::println!("[darwin] thread_selfid: tid={:#x}", tid);
    trapframe.set_return_value(tid as usize);
    tid as usize
}

pub fn sys_proc_info(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    trapframe.set_return_value(0);
    0
}

pub fn sys_getentropy(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let buf = trapframe.get_arg(0);
    let size = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);

    if size > 256 {
        trapframe.set_return_value(EINVAL);
        return EINVAL;
    }

    if let Some(kva) = task.vm_manager.translate_to_kva(buf) {
        let bytes = unsafe { core::slice::from_raw_parts_mut(kva as *mut u8, size) };
        for b in bytes.iter_mut() {
            *b = 0x42;
        }
        trapframe.set_return_value(0);
        0
    } else {
        trapframe.set_return_value(EFAULT);
        EFAULT
    }
}

pub fn sys_getlogin(_abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let namebuf = trapframe.get_arg(0);
    let _namelen = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);

    let login = b"root\0";
    if let Some(kva) = task.vm_manager.translate_to_kva(namebuf) {
        let dst = unsafe { core::slice::from_raw_parts_mut(kva as *mut u8, login.len()) };
        dst.copy_from_slice(login);
        trapframe.set_return_value(0);
        0
    } else {
        trapframe.set_return_value(EFAULT);
        EFAULT
    }
}
