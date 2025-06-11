use alloc::{boxed::Box, string::ToString, vec::Vec};
use crate::{abi::xv6::riscv64::fs::xv6fs::Stat, arch::{self, Registers, Trapframe}, device::manager::DeviceManager, fs::{helper::get_path_str, DeviceFileInfo, FileType, SeekFrom, VfsManager}, library::std::string::{cstring_to_string, StringConversionError}, task::{elf_loader::load_elf_into_task, mytask}, vm};

const MAX_PATH_LENGTH: usize = 128;

pub fn sys_exec(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    // Get arguments from the trapframe
    let path_ptr = task.vm_manager
        .translate_vaddr(trapframe.get_arg(0))
        .unwrap() as *const u8;
    
    /* 
     * The second and third arguments are pointers to arrays of pointers to
     * null-terminated strings (char **argv).
     * We will not use them in this implementation.
     */
    let mut argv_ptr = task.vm_manager
        .translate_vaddr(trapframe.get_arg(1))
        .unwrap() as *const *const u8;
    
    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);

    task.vcpu.store(trapframe); // Store the current trapframe in the task's vcpu

    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => VfsManager::to_absolute_path(&task, &s).unwrap(),
        Err(_) => {
            return usize::MAX; // Invalid C string
        },
    };

    // Get argv from argv_ptr
    let mut argv = Vec::new();

    loop {
        if argv_ptr.is_null() {
            break; // Stop if we reach a null pointer
        }

        let translated_argv_ptr = task.vm_manager
            .translate_vaddr( unsafe { *argv_ptr } as usize )
            .unwrap_or(0) as *const u8;

        let (arg, _) = match cstring_to_string(translated_argv_ptr, 32) {
            Ok((s, len)) => (s, len),
            Err(e) => {
                match e {
                    StringConversionError::NullPointer => {
                        break; // Stop if we reach a null pointer
                    }
                    _ => {
                        return usize::MAX; // Invalid C string in argv
                    }
                }
            },
        };

        if arg.is_empty() {
            break; // Stop if we reach a null pointer
        }
        argv.push(arg);
        argv_ptr = unsafe { argv_ptr.add(1) };
    }
    
    // Backup the managed pages
    let mut backup_pages = Vec::new();
    backup_pages.append(&mut task.managed_pages); // Move the pages to the backup
    // Backup the vm mapping
    let backup_vm_mapping = task.vm_manager.remove_all_memory_maps(); // Move the memory mapping to the backup
    // Backing up the size
    let backup_text_size = task.text_size;
    let backup_data_size = task.data_size;
        
    // Try to open the executable file
    let file = match task.vfs.as_ref() {
        Some(vfs) => vfs.open(path_str.as_str(), 0),
        None => {
            // Restore the managed pages, memory mapping and sizes
            task.managed_pages = backup_pages; // Restore the pages
            task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
            task.text_size = backup_text_size; // Restore the text size
            task.data_size = backup_data_size; // Restore the data size
            return usize::MAX; // VFS not initialized
        }
    };
    
    if file.is_err() {
        // Restore the managed pages, memory mapping and sizes
        task.managed_pages = backup_pages; // Restore the pages
        task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
        task.text_size = backup_text_size; // Restore the text size
        task.data_size = backup_data_size; // Restore the data size
        return usize::MAX; // File open error
    }

    let mut file = file.unwrap();

    task.text_size = 0;
    task.data_size = 0;
    task.stack_size = 0;
    
    // Load the ELF file and replace the current process
    match load_elf_into_task(&mut file, task) {
        Ok(entry_point) => {
            // Set the name
            task.name = path_str.to_string();
            // Clear page table entries
            let idx = arch::vm::get_root_pagetable_ptr(task.vm_manager.get_asid()).unwrap();
            let root_page_table = arch::vm::get_pagetable(idx).unwrap();
            root_page_table.unmap_all();
            // Setup the trapframe
            vm::setup_trampoline(&mut task.vm_manager);
            // Setup the stack
            let (_, stack_top) = vm::setup_user_stack(task);
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

            // Switch to the new task
            task.vcpu.switch(trapframe);
            
            // Return argc as the return value (a0)
            argc
        },
        Err(_) => {
            // Restore the managed pages, memory mapping and sizes
            task.managed_pages = backup_pages; // Restore the pages
            task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
            task.text_size = backup_text_size; // Restore the text size
            task.data_size = backup_data_size; // Restore the data size

            // Return error code
            usize::MAX
        }
    }
}

#[repr(i32)]
enum OpenMode {
    ReadOnly  = 0x000,
    WriteOnly = 0x001,
    ReadWrite = 0x002,
    Create    = 0x200,
    Truncate  = 0x400,
}

pub fn sys_open(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let mode = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop if open fails
    trapframe.increment_pc_next(task);

    // Convert path bytes to string
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => VfsManager::to_absolute_path(&task, &path).unwrap(),
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Try to open the file
    let file = match task.vfs.as_ref() {
        Some(vfs) => vfs.open(&path_str, 0),
        None => return usize::MAX, // VFS not initialized
    };

    match file {
        Ok(file) => {
            // Register the file with the task
            let fd = task.add_file(file);
            // println!("Opened file: {} with fd: {}", path_str, fd.unwrap_or(usize::MAX));
            if fd.is_err() {
                return usize::MAX; // File descriptor error
            }
            fd.unwrap() as usize
        }
        Err(e) =>{
            // If the file does not exist and we are trying to create it
            if mode & OpenMode::Create as i32 != 0 {
                let vfs = task.vfs.as_ref().unwrap();
                let res = vfs.create_file(&path_str, FileType::RegularFile);
                if res.is_err() {
                    return usize::MAX; // File creation error
                }
                match vfs.open(&path_str, 0) {
                    Ok(file) => {
                        // Register the file with the task
                        let fd = task.add_file(file);
                        if fd.is_err() {
                            return usize::MAX; // File descriptor error
                        }
                        fd.unwrap() as usize
                    }
                    Err(_) => usize::MAX, // File open error
                }
            } else {
                return usize::MAX; // VFS not initialized
            }
        }
    }
}

pub fn sys_dup(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);

    if let Some(old_file) = task.get_file(fd) {
        let file = old_file.clone();
        let new_fd = task.add_file(file);
        if new_fd.is_ok() {
            return new_fd.unwrap() as usize;
        } else {
            return usize::MAX; // File descriptor error
        }
    }
    usize::MAX // -1
}

pub fn sys_close(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);
    if task.remove_file(fd).is_ok() {
        0
    } else {
        usize::MAX // -1
    }
}

pub fn sys_read(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *mut u8;
    let count = trapframe.get_arg(2) as usize;

    let file = task.get_mut_file(fd);
    if file.is_none() {
        // Increment PC to avoid infinite loop if read fails
        trapframe.increment_pc_next(task);
        return usize::MAX; // Invalid file descriptor
    }

    let file = file.unwrap();

    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
    
    match file.read(buffer) {
        Ok(n) => {
            // Increment PC to avoid infinite loop if read fails
            trapframe.increment_pc_next(task);
            n
        }
        Err(_) => {
            // Increment PC to avoid infinite loop if read fails
            trapframe.increment_pc_next(task);
            usize::MAX // Read error;
        }
    }
}

pub fn sys_write(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(task);

    let file = task.get_mut_file(fd);
    if file.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let file = file.unwrap();

    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
    
    match file.write(buffer) {
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
    let fd = trapframe.get_arg(0) as usize;
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if lseek fails
    trapframe.increment_pc_next(task);

    let file = task.get_mut_file(fd);
    if file.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let file = file.unwrap();
    let whence  = match whence {
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

// Create device file
pub fn sys_mknod(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    let name_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let name = get_path_str(name_ptr).unwrap();
    let path = VfsManager::to_absolute_path(&task, &name).unwrap();

    let major = trapframe.get_arg(1) as u32;
    let minor = trapframe.get_arg(2) as u32;

    match (major, minor) {
        (1, 0) => {
            // Create a console device
            let console_dev = Some(DeviceManager::get_mut_manager().register_device(Box::new(
                crate::abi::xv6::drivers::console::ConsoleDevice::new(0, "console")
            )));
        
            let vfs = task.vfs.as_mut().unwrap();
            let res = vfs.create_file(&path, FileType::CharDevice(
                DeviceFileInfo {
                    device_id: console_dev.unwrap(),
                    device_type: crate::device::DeviceType::Char,
                }
            ));
            // println!("Created console device at {}", path);
        },
        _ => {},
    }
    0
}


pub fn sys_fstat(trapframe: &mut crate::arch::Trapframe) -> usize {
    let fd = trapframe.get_arg(0) as usize;

    let task = mytask()
        .expect("sys_fstat: No current task found");
    trapframe.increment_pc_next(task); // Increment the program counter

    let stat_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1) as usize)
        .expect("sys_fstat: Failed to translate stat pointer") as *mut Stat;
    let file = match task.get_file(fd) {
        Some(file) => file,
        None => return usize::MAX, // Return -1 on error
    };
    let metadata = file.metadata()
        .expect("sys_fstat: Failed to get file metadata");

    if stat_ptr.is_null() {
        return usize::MAX; // Return -1 if stat pointer is null
    }
    
    let stat = unsafe { &mut *stat_ptr };

    *stat = Stat {
        dev: 0,
        ino: metadata.file_id as u32,
        file_type: match metadata.file_type {
            FileType::Directory => 1, // T_DIR
            FileType::RegularFile => 2,      // T_FILE
            FileType::CharDevice(_) => 3, // T_DEVICE
            FileType::BlockDevice(_) => 3, // T_DEVICE
            _ => 0, // Unknown type
        },
        nlink: 1,
        size: metadata.size as u64,
    };

    0
}

pub fn sys_mkdir(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let path = match get_path_str(path_ptr) {
        Ok(p) => VfsManager::to_absolute_path(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    // Try to create the directory
    let vfs = task.vfs.as_mut().unwrap();
    match vfs.create_dir(&path) {
        Ok(_) => 0, // Success
        Err(_) => usize::MAX, // Error
    }
}