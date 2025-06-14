//! Task-related system call implementations.
//!
//! This module implements system calls that interact with task management,
//! filesystem operations, and process control. Many operations leverage
//! the VfsManager for filesystem access when tasks have isolated namespaces.
//!
//! # VfsManager Integration
//!
//! System calls automatically use the task's VfsManager when available:
//! - Tasks with `vfs: Some(Arc<VfsManager>)` use their isolated filesystem namespace
//! - Tasks with `vfs: None` fall back to global filesystem operations
//! - Bind mount operations enable controlled sharing between isolated namespaces
//! - All filesystem operations are thread-safe and handle concurrent access properly

use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::str;

use crate::abi::xv6::riscv64::fs::xv6fs::Xv6FSParams;
use crate::abi::{AbiRegistry, MAX_ABI_LENGTH};
use crate::device::manager::DeviceManager;
use crate::fs::{FileType, VfsManager, MAX_PATH_LENGTH};
use crate::task::elf_loader::load_elf_into_task;

use crate::arch::{get_cpu, vm, Registers, Trapframe};
use crate::print;
use crate::sched::scheduler::get_scheduler;
use crate::task::{CloneFlags, WaitError};
use crate::vm::{setup_user_stack, setup_trampoline};

use super::mytask;

pub fn sys_brk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let brk = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);
    match task.set_brk(brk) {
        Ok(_) => task.get_brk(),
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_sbrk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0);
    let brk = task.get_brk();
    trapframe.increment_pc_next(task);
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_putchar(trapframe: &mut Trapframe) -> usize {
    let c = trapframe.get_arg(0) as u32;
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    if let Some(ch) = char::from_u32(c) {
        print!("{}", ch);
    } else {
        return usize::MAX; // -1
    }
    0
}

pub fn sys_getchar(trapframe: &mut Trapframe) -> usize {
    let serial = DeviceManager::get_mut_manager().basic.borrow_mut_serial(0).unwrap();
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    if let Some(byte) = serial.get() {
        byte as usize
    } else {
        0 // Return 0 if no data available
    }
}

pub fn sys_exit(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    get_scheduler().schedule(get_cpu());
    trapframe.get_arg(0) as usize
}

pub fn sys_clone(trapframe: &mut Trapframe) -> usize {
    let parent_task = mytask().unwrap();
    trapframe.increment_pc_next(parent_task); /* Increment the program counter */
    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.store(trapframe);
    let clone_flags = CloneFlags::from_raw(trapframe.get_arg(0) as u64);

    /* Clone the task */
    match parent_task.clone_task(clone_flags) {
        Ok(mut child_task) => {
            let child_id = child_task.get_id();
            child_task.vcpu.regs.reg[10] = 0; /* Set the return value to 0 in the child task */
            get_scheduler().add_task(child_task, get_cpu().get_cpuid());
            /* Return the child task ID to the parent task */
            child_id
        },
        Err(_) => {
            usize::MAX /* Return -1 on error */
        }
    }
}

pub fn sys_execve(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    // Get arguments from the trapframe
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    
    /* 
     * The second and third arguments are pointers to arrays of pointers to
     * null-terminated strings (char **argv, char **envp).
     * We will not use them in this implementation.
     */
    // let argv_ptr = trapframe.get_arg(1) as *const *const u8;
    // let envp_ptr = trapframe.get_arg(2) as *const *const u8;
    
    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);
    
    // Get the current task
    let task = mytask().unwrap();

    // Backup the managed pages
    let mut backup_pages = Vec::new();
    backup_pages.append(&mut task.managed_pages); // Move the pages to the backup
    // Backup the vm mapping
    let backup_vm_mapping = task.vm_manager.remove_all_memory_maps(); // Move the memory mapping to the backup
    // Backing up the size
    let backup_text_size = task.text_size;
    let backup_data_size = task.data_size;
    
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
            
            // Safety check to prevent infinite loop
            if i > MAX_PATH_LENGTH {
                // Restore the managed pages, memory mapping and sizes
                task.managed_pages = backup_pages; // Restore the pages
                task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
                task.text_size = backup_text_size; // Restore the text size
                task.data_size = backup_data_size; // Restore the data size
                return usize::MAX; // Path too long
            }
        }
    }
    
    // Convert path bytes to string
    let path_str = match str::from_utf8(&path_bytes) {
        Ok(s) => match VfsManager::to_absolute_path(&task, s) {
            Ok(path) => path,
            Err(_) => {
                // Restore the managed pages, memory mapping and sizes
                task.managed_pages = backup_pages; // Restore the pages
                task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
                task.text_size = backup_text_size; // Restore the text size
                task.data_size = backup_data_size; // Restore the data size
                return usize::MAX; // Path error
            }
        },
        Err(_) => {
            // Restore the managed pages, memory mapping and sizes
            task.managed_pages = backup_pages; // Restore the pages
            task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
            task.text_size = backup_text_size; // Restore the text size
            task.data_size = backup_data_size; // Restore the data size
            return usize::MAX // Invalid UTF-8
        },
    };
    
    // Ensure that task.vfs is initialized before proceeding.
    if task.vfs.is_none() {
        // Restore the managed pages, memory mapping and sizes
        task.managed_pages = backup_pages; // Restore the pages
        task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
        task.text_size = backup_text_size; // Restore the text size
        task.data_size = backup_data_size; // Restore the data size
        return usize::MAX; // VFS not initialized
    }
    
    // Try to open the executable file
    let file = match task.vfs.as_ref() {
        Some(vfs) => vfs.open(&path_str, 0),
        None => {
            // Restore the managed pages, memory mapping and sizes
            task.managed_pages = backup_pages; // Restore the pages
            task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
            task.text_size = backup_text_size; // Restore the text size
            task.data_size = backup_data_size; // Restore the data size
            return usize::MAX; // VFS uninitialized
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
    let file_obj = file.unwrap();
    // file_obj is already a KernelObject::File
    let file_ref = match file_obj.as_file() {
        Some(file) => file,
        None => {
            // Restore the managed pages, memory mapping and sizes
            task.managed_pages = backup_pages;
            task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap();
            task.text_size = backup_text_size;
            task.data_size = backup_data_size;
            return usize::MAX; // Failed to get file reference
        }
    };

    task.text_size = 0;
    task.data_size = 0;
    task.stack_size = 0;
    
    // Load the ELF file and replace the current process
    match load_elf_into_task(file_ref, task) {
        Ok(entry_point) => {
            // Set the name
            task.name = path_str;
            // Clear page table entries
            let root_page_table  = vm::get_root_pagetable(task.vm_manager.get_asid()).unwrap();
            root_page_table.unmap_all();
            // Setup the trapframe
            setup_trampoline(&mut task.vm_manager);
            // Setup the stack
            let stack_pointer = setup_user_stack(task).1;

            // Set the new entry point for the task
            task.set_entry_point(entry_point as usize);
            
            // Reset task's registers (except for those needed for arguments)
            task.vcpu.regs = Registers::new();
            // Set the stack pointer
            task.vcpu.set_sp(stack_pointer);

            // Switch to the new task
            task.vcpu.switch(trapframe);
            
            // Return 0 on success (though this should never actually return)
            0
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

pub fn sys_execve_abi(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let abi_str_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(3)).unwrap() as *const u8;
    let mut abi_bytes = Vec::new();
    let mut i = 0;
    unsafe {
        loop {
            let byte = *abi_str_ptr.add(i);
            if byte == 0 {
                break;
            }
            abi_bytes.push(byte);
            i += 1;
            
            // Safety check to prevent infinite loop
            if i > MAX_ABI_LENGTH {
                trapframe.increment_pc_next(task);
                return usize::MAX; // Path too long
            }
        }
    }
    // Convert abi bytes to string
    let abi_str = match str::from_utf8(&abi_bytes) {
        Ok(s) => s,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    let abi = AbiRegistry::instantiate(abi_str);
    if abi.is_none() {
        trapframe.increment_pc_next(task);
        return usize::MAX; // ABI not found
    }
    let abi = abi.unwrap();
    // let backup_abi = task.abi.take();
    // let backup_vfs = task.vfs.take();

    let res = sys_execve(trapframe);

    if res != usize::MAX {
        // match abi.init_fs() {
        //     Some(vfs) => {
        //         task.vfs = Some(Arc::new(vfs));
        //     },
        //     None => {}
        // }
        task.abi = Some(abi);
    }

    res
}

pub fn sys_waitpid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let pid = trapframe.get_arg(0) as i32;
    let status_ptr = trapframe.get_arg(1) as *mut i32;
    let _options = trapframe.get_arg(2) as i32; // Not used in this implementation

    if pid == -1 {
        for pid in task.get_children().clone() {
            match task.wait(pid) {
                Ok(status) => {
                    // If the child task is exited, we can return the status
                    if status_ptr != core::ptr::null_mut() {
                        let status_ptr = task.vm_manager.translate_vaddr(status_ptr as usize).unwrap() as *mut i32;
                        unsafe {
                            *status_ptr = status;
                        }
                    }
                    trapframe.increment_pc_next(task);
                    return pid;
                },
                Err(error) => {
                    match error {
                        WaitError::ChildNotExited(_) => continue,
                        _ => {
                            trapframe.increment_pc_next(task);
                            return usize::MAX;
                        },
                    }
                }
            }
        }
        // Any child process has exited
        trapframe.increment_pc_next(task);
        return usize::MAX;
    }
    
    match task.wait(pid as usize) {
        Ok(status) => {
            // If the child task is exited, we can return the status
            if status_ptr != core::ptr::null_mut() {
                let status_ptr = task.vm_manager.translate_vaddr(status_ptr as usize).unwrap() as *mut i32;
                unsafe {
                    *status_ptr = status;
                }
            }
            trapframe.increment_pc_next(task);
            pid as usize
        }
        Err(error) => {
            match error {
                WaitError::NoSuchChild(_) => {
                    trapframe.increment_pc_next(task);
                    usize::MAX
                },
                WaitError::ChildTaskNotFound(_) => {
                    trapframe.increment_pc_next(task);
                    usize::MAX
                },
                WaitError::ChildNotExited(_) => {
                    // If the child task is not exited, we need to wait for it
                    get_scheduler().schedule(trapframe);
                    trapframe.get_return_value()
                },
            }
        }
    }
}

pub fn sys_getpid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_id() as usize
}

pub fn sys_getppid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_parent_id().unwrap_or(task.get_id()) as usize
}