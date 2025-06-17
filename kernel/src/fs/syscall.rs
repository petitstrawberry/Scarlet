use alloc::vec::Vec;

use crate::{arch::Trapframe, task::mytask};

use super::{SeekFrom, VfsManager, MAX_PATH_LENGTH};

pub fn sys_open(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let _flags = trapframe.get_arg(1) as i32;
    let _mode = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if open fails
    trapframe.increment_pc_next(task);

    // Parse path as a null-terminated C string
    let mut path_bytes = Vec::new();
    let mut i = 0;
    unsafe {
        loop {
            let byte = *path_ptr.add(i);
            if byte == 0 {
                break;
            }
            path_bytes.push(byte);
            i += 1;

            if i > MAX_PATH_LENGTH {
                return usize::MAX; // Path too long
            }
        }
    }

    // Convert path bytes to string
    let path_str = match str::from_utf8(&path_bytes) {
        Ok(s) => VfsManager::to_absolute_path(&task, s).unwrap(),
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Try to open the file using VFS
    let vfs = match task.get_vfs() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };
    let file_obj = vfs.open(&path_str, 0);
    match file_obj {
        Ok(kernel_obj) => {
            // file_obj is already a KernelObject::File
            let handle = task.handle_table.insert(kernel_obj);
            match handle {
                Ok(handle) => handle as usize,
                Err(_) => usize::MAX, // Handle table full
            }
        }
        Err(_) => usize::MAX, // File open error
    }
}

pub fn sys_close(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    trapframe.increment_pc_next(task);
    if task.handle_table.remove(fd).is_some() {
        0
    } else {
        usize::MAX // -1
    }
}

pub fn sys_read(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *mut u8;
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if read fails
    trapframe.increment_pc_next(task);

    let kernel_obj = task.handle_table.get(fd);
    if kernel_obj.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let kernel_obj = kernel_obj.unwrap();
    let stream = kernel_obj.as_stream();
    if stream.is_none() {
        return usize::MAX; // Object doesn't support stream operations
    }

    let stream = stream.unwrap();
    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
    
    match stream.read(buffer) {
        Ok(n) => {
            n
        }
        Err(_) => {
            return usize::MAX; // Read error
        }
    }
}

pub fn sys_write(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(task);

    let kernel_obj = task.handle_table.get(fd);
    if kernel_obj.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let kernel_obj = kernel_obj.unwrap();
    let stream = kernel_obj.as_stream();
    if stream.is_none() {
        return usize::MAX; // Object doesn't support stream operations
    }

    let stream = stream.unwrap();
    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
    
    match stream.write(buffer) {
        Ok(n) => {
            n
        }
        Err(_) => {
            return usize::MAX; // Write error
        }
    }
}

pub fn sys_lseek(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if lseek fails
    trapframe.increment_pc_next(task);

    let kernel_obj = task.handle_table.get(fd);
    if kernel_obj.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let kernel_obj = kernel_obj.unwrap();
    let file = kernel_obj.as_file();
    if file.is_none() {
        return usize::MAX; // Object doesn't support file operations
    }

    let file = file.unwrap();
    let whence = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return usize::MAX, // Invalid whence
    };

    match file.seek(whence) {
        Ok(pos) => {
            pos as usize
        }
        Err(_) => {
            return usize::MAX; // Lseek error
        }
    }
}