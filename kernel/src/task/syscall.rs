use alloc::string::ToString;
use alloc::vec::Vec;
use core::{error, str};

use crate::device::manager::DeviceManager;
use crate::fs::{File, MAX_PATH_LENGTH};
use crate::task::elf_loader::load_elf_into_task;

use crate::arch::{get_cpu, vm, Registers, Trapframe};
use crate::print;
use crate::sched::scheduler::get_scheduler;
use crate::task::WaitError;
use crate::vm::{setup_user_stack, setup_trampoline};

use super::mytask;

pub fn sys_brk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let brk = trapframe.get_arg(0);
    trapframe.epc += 4;
    match task.set_brk(brk) {
        Ok(_) => task.get_brk(),
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_sbrk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0);
    let brk = task.get_brk();
    trapframe.epc += 4;
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_putchar(trapframe: &mut Trapframe) -> usize {
    let c = trapframe.get_arg(0) as u32;
    trapframe.epc += 4;
    if let Some(ch) = char::from_u32(c) {
        print!("{}", ch);
    } else {
        return usize::MAX; // -1
    }
    0
}

pub fn sys_getchar(trapframe: &mut Trapframe) -> usize {
    let serial = DeviceManager::get_mut_manager().basic.borrow_mut_serial(0).unwrap();
    if let Some(c) = serial.read_byte() {
        trapframe.epc += 4;
        return c as usize;
    } else {
        trapframe.get_return_value()
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
    
    trapframe.epc += 4; /* Increment the program counter */

    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.store(trapframe);
    
    /* Clone the task */
    match parent_task.clone_task() {
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
    trapframe.epc += 4;
    
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
        Ok(s) => s,
        Err(_) => {
            // Restore the managed pages, memory mapping and sizes
            task.managed_pages = backup_pages; // Restore the pages
            task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
            task.text_size = backup_text_size; // Restore the text size
            task.data_size = backup_data_size; // Restore the data size
            return usize::MAX // Invalid UTF-8
        },
    };
    
    // Try to open the executable file
    let mut file = File::new(path_str.to_string());
    if file.open(0).is_err() {
        // Restore the managed pages, memory mapping and sizes
        task.managed_pages = backup_pages; // Restore the pages
        task.vm_manager.restore_memory_maps(backup_vm_mapping).unwrap(); // Restore the memory mapping
        task.text_size = backup_text_size; // Restore the text size
        task.data_size = backup_data_size; // Restore the data size
        return usize::MAX; // File open error
    }

    task.text_size = 0;
    task.data_size = 0;
    task.stack_size = 0;
    
    // Load the ELF file and replace the current process
    match load_elf_into_task(&mut file, task) {
        Ok(entry_point) => {
            // Set the name
            task.name = path_str.to_string();
            // Clear page table entries
            let idx = vm::get_root_page_table_idx(task.vm_manager.get_asid()).unwrap();
            let root_page_table = vm::get_page_table(idx).unwrap();
            root_page_table.unmap_all();
            // Setup the trapframe
            setup_trampoline(&mut task.vm_manager);
            // Setup the stack
            let stack_pointer = setup_user_stack(task);

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
                    trapframe.epc += 4;
                    return pid;
                },
                Err(error) => {
                    match error {
                        WaitError::ChildNotExited(_) => continue,
                        _ => {
                            trapframe.epc += 4;
                            return usize::MAX;
                        },
                    }
                }
            }
        }
        // Any child process has exited
        trapframe.epc += 4;
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
            trapframe.epc += 4;
            pid as usize
        }
        Err(error) => {
            match error {
                WaitError::NoSuchChild(_) => {
                    trapframe.epc += 4;
                    usize::MAX
                },
                WaitError::ChildTaskNotFound(_) => {
                    trapframe.epc += 4;
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
    trapframe.epc += 4;
    task.get_id() as usize
}

pub fn sys_getppid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.epc += 4;
    task.get_parent_id().unwrap_or(task.get_id()) as usize
}