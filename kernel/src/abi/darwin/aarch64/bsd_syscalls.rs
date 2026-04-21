use alloc::string::String;

use crate::abi::darwin::aarch64::DarwinAarch64Abi;
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
