use alloc::{string::String, vec::Vec, vec};

use crate::{arch::Trapframe, library::std::string::cstring_to_string, task::mytask};

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

pub fn sys_truncate(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let length = trapframe.get_arg(1) as u64;
    
    trapframe.increment_pc_next(task);

    // Convert path bytes to string
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match VfsManager::to_absolute_path(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };
    
    match vfs.truncate(&path_str, length) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

pub fn sys_ftruncate(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32;
    let length = trapframe.get_arg(1) as u64;
    
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
    match file.truncate(length) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

pub fn sys_bind_mount(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let source_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let target_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let flags = trapframe.get_arg(2) as u32;

    trapframe.increment_pc_next(task);

    // Convert source and target paths to strings
    let source_str = match cstring_to_string(source_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    
    let target_str = match cstring_to_string(target_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Perform the bind mount using VFS
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };

    match vfs.bind_mount(&source_str, &target_str, flags == 1) { // Assuming 1 means read-only
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

pub fn sys_overlay_mount(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let upperdir_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let lowerdir_count = trapframe.get_arg(1) as usize;
    let lowerdirs_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(2)).unwrap() as *const u8;
    let target_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(3)).unwrap() as *const u8;

    trapframe.increment_pc_next(task);

    // Convert paths to strings
    let upperdir_str = match cstring_to_string(upperdir_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    let target_str = match cstring_to_string(target_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    // Collect lower directories
    let mut lowerdirs = Vec::new();
    for i in 0..lowerdir_count {
        let lowerdir = unsafe { lowerdirs_ptr.add(i * MAX_PATH_LENGTH) };
        match cstring_to_string(lowerdir, MAX_PATH_LENGTH) {
            Ok((s, _)) => lowerdirs.push(s),
            Err(_) => return usize::MAX, // Invalid UTF-8
        }
    }

    // Perform the overlay mount using VFS
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };

    let lowerdirs_refs: Vec<&str> = lowerdirs.iter().map(|s| s.as_str()).collect();
    return match vfs.overlay_mount(Some(&upperdir_str), lowerdirs_refs, &target_str) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}