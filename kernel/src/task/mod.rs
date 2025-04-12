//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

pub mod syscall;
pub mod elf_loader;

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

use crate::{arch::{get_cpu, vcpu::Vcpu}, environment::{DEAFAULT_MAX_TASK_DATA_SIZE, DEAFAULT_MAX_TASK_STACK_SIZE, DEAFAULT_MAX_TASK_TEXT_SIZE, KERNEL_VM_STACK_END, PAGE_SIZE}, mem::page::{allocate_pages, free_pages, Page}, sched::scheduler::get_scheduler, vm::{manager::VirtualMemoryManager, user_kernel_vm_init, user_vm_init, vmem::{MemoryArea, VirtualMemoryMap, VirtualMemorySegment}}};


#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TaskState {
    NotInitialized,
    Ready,
    Running,
    Blocked,
    Terminated,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TaskType {
    Kernel,
    User,
}

#[derive(Debug, Clone)]
pub struct Task {
    id: usize,
    pub name: String,
    pub priority: u32,
    pub vcpu: Vcpu,
    pub state: TaskState,
    pub task_type: TaskType,
    pub entry: usize,
    pub stack_size: usize, /* Size of the stack in bytes */
    pub data_size: usize, /* Size of the data segment in bytes (NOT work in Kernel task) */
    pub text_size: usize, /* Size of the text segment in bytes (NOT work in Kernel task) */
    pub max_stack_size: usize, /* Maximum size of the stack in bytes */
    pub max_data_size: usize, /* Maximum size of the data segment in bytes */
    pub max_text_size: usize, /* Maximum size of the text segment in bytes */
    pub vm_manager: VirtualMemoryManager,
}

static TASK_ID: Mutex<usize> = Mutex::new(0);

impl Task {
    pub fn new(name: String, priority: u32, task_type: TaskType) -> Self {
        let mut taskid = TASK_ID.lock();
        let task = Task {
            id: *taskid,
            name,
            priority,
            vcpu: Vcpu::new(),
            state: TaskState::NotInitialized,
            task_type,
            entry: 0,
            stack_size: 0,
            data_size: 0,
            text_size: 0,
            max_stack_size: DEAFAULT_MAX_TASK_STACK_SIZE,
            max_data_size: DEAFAULT_MAX_TASK_DATA_SIZE,
            max_text_size: DEAFAULT_MAX_TASK_TEXT_SIZE,
            vm_manager: VirtualMemoryManager::new(),
        };
        *taskid += 1;
        task
    }
    
    pub fn init(&mut self) {
        match self.task_type {
            TaskType::Kernel => {
                user_kernel_vm_init(self);
                /* Set sp to the top of the kernel stack */
                self.vcpu.regs.reg[2] = KERNEL_VM_STACK_END + 1;
            },
            TaskType::User => user_vm_init(self),
        }
        
        /* Set the task state to Ready */
        self.state = TaskState::Ready;
    }

    pub fn get_id(&self) -> usize {
        self.id
    }

   /// Get the size of the task.
   /// 
   /// # Returns
   /// The size of the task in bytes.
    pub fn get_size(&self) -> usize {
        self.stack_size + self.text_size + self.data_size
    }

    /// Get the program break (NOT work in Kernel task)
    /// 
    /// # Returns
    /// The program break address
    pub fn get_brk(&self) -> usize {
        self.text_size + self.data_size
    }

    /// Set the program break (NOT work in Kernel task)
    /// 
    /// # Arguments
    /// * `brk` - The new program break address
    /// 
    /// # Returns
    /// If successful, returns Ok(()), otherwise returns an error.
    pub fn set_brk(&mut self, brk: usize) -> Result<(), &'static str> {
        // println!("New brk: {:#x}", brk);
        if brk < self.text_size {
            return Err("Invalid address");
        }
        let prev_brk = self.get_brk();
        if brk < prev_brk {
            /* Free pages */
            /* Round address to the page boundary */
            let prev_addr = (prev_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let addr = (brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let num_of_pages = (prev_addr - addr) / PAGE_SIZE;
            
            self.free_pages(addr, num_of_pages);
            
        } else if brk > prev_brk {
            /* Allocate pages */
            /* Round address to the page boundary */
            let prev_addr = (prev_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let addr = (brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let num_of_pages = (addr - prev_addr) / PAGE_SIZE;

            if num_of_pages > 0 {
                match self.vm_manager.search_memory_map(prev_addr) {
                    Some(_) => {},
                    None => {
                        match self.allocate_pages(prev_addr, num_of_pages, VirtualMemorySegment::Data) {
                            Ok(_) => {},
                            Err(_) => return Err("Failed to allocate pages"),
                        }
                    },
                }
            }
        }

        self.data_size = brk - self.text_size;
        Ok(())
    }

    /// Allocate pages for the task.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    /// * `segment` - The segment type to allocate pages
    /// 
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    /// 
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    pub fn allocate_pages(&mut self, vaddr: usize, num_of_pages: usize, segment: VirtualMemorySegment) -> Result<VirtualMemoryMap, &'static str> {

        if vaddr % PAGE_SIZE != 0 {
            return Err("Address is not page aligned");
        }
        
        let permissions = segment.get_permissions();
        let pages = allocate_pages(num_of_pages);
        let size = num_of_pages * PAGE_SIZE;
        let paddr = pages as usize;
        let mmap = VirtualMemoryMap {
            pmarea: MemoryArea {
                start: paddr,
                end: paddr + size - 1,
            },
            vmarea: MemoryArea {
                start: vaddr,
                end: vaddr + size - 1,
            },
            permissions,
        };
        self.vm_manager.add_memory_map(mmap);
        match segment {
            VirtualMemorySegment::Stack => self.stack_size += size,
            VirtualMemorySegment::Heap => self.data_size += size,
            VirtualMemorySegment::Bss => self.data_size += size,
            VirtualMemorySegment::Data => self.data_size += size,
            VirtualMemorySegment::Text => self.text_size += size,
        }
        // println!("Allocated pages: {:#x} - {:#x}", vaddr, vaddr + size - 1);
        Ok(mmap)
    }

    /// Free pages for the task.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    pub fn free_pages(&mut self, vaddr: usize, num_of_pages: usize) {
        let page = vaddr / PAGE_SIZE;
        for p in 0..num_of_pages {
            let vaddr = (page + p) * PAGE_SIZE;
            match self.vm_manager.search_memory_map_idx(vaddr) {
                Some(idx) => {
                    let mmap = self.vm_manager.remove_memory_map(idx).unwrap();
                    if p == 0 && mmap.vmarea.start < vaddr {
                        /* Re add the first part of the memory map */
                        let size = vaddr - mmap.vmarea.start;
                        let paddr = mmap.pmarea.start;
                        let mmap1 = VirtualMemoryMap {
                            pmarea: MemoryArea {
                                start: paddr,
                                end: paddr + size - 1,
                            },
                            vmarea: MemoryArea {
                                start: mmap.vmarea.start,
                                end: vaddr - 1,
                            },
                            permissions: mmap.permissions,
                        };
                        self.vm_manager.add_memory_map(mmap1);
                        // println!("Removed map : {:#x} - {:#x}", mmap.vmarea.start, mmap.vmarea.end);
                        // println!("Re added map: {:#x} - {:#x}", mmap1.vmarea.start, mmap1.vmarea.end);
                    }
                    if p == num_of_pages - 1 && mmap.vmarea.end > vaddr + PAGE_SIZE - 1 {
                        /* Re add the second part of the memory map */
                        let size = mmap.vmarea.end - (vaddr + PAGE_SIZE) + 1;
                        let paddr = mmap.pmarea.start + (vaddr + PAGE_SIZE - mmap.vmarea.start);
                        let mmap2 = VirtualMemoryMap {
                            pmarea: MemoryArea {
                                start: paddr,
                                end: paddr + size - 1,
                            },
                            vmarea: MemoryArea {
                                start: vaddr + PAGE_SIZE,
                                end: mmap.vmarea.end,
                            },
                            permissions: mmap.permissions,
                        };
                        self.vm_manager.add_memory_map(mmap2);
                        // println!("Removed map : {:#x} - {:#x}", mmap.vmarea.start, mmap.vmarea.end);
                        // println!("Re added map: {:#x} - {:#x}", mmap2.vmarea.start, mmap2.vmarea.end);
                    }
                    let offset = vaddr - mmap.vmarea.start;
                    free_pages((mmap.pmarea.start + offset) as *mut Page, 1);
                    // println!("Freed pages : {:#x} - {:#x}", vaddr, vaddr + PAGE_SIZE - 1);
                },
                None => {},
            }
        }
        /* Unmap pages */
        let root_pagetable = self.vm_manager.get_root_page_table().unwrap();
        for p in 0..num_of_pages {
            let vaddr = (page + p) * PAGE_SIZE;
            root_pagetable.unmap(vaddr);
        }
    }

    // Set the entry point
    pub fn set_entry_point(&mut self, entry: usize) {
        self.vcpu.set_pc(entry as u64);
    }
}

/// Create a new kernel task.
/// 
/// # Arguments
/// * `name` - The name of the task
/// * `priority` - The priority of the task
/// * `func` - The function to run in the task
/// 
/// # Returns
/// The new task.
pub fn new_kernel_task(name: String, priority: u32, func: fn()) -> Task {
    let mut task = Task::new(name, priority, TaskType::Kernel);
    task.entry = func as usize;
    task
}

/// Create a new user task.
/// 
/// # Arguments
/// * `name` - The name of the task
/// * `priority` - The priority of the task
/// 
/// # Returns
/// The new task.
pub fn new_user_task(name: String, priority: u32) -> Task {
    Task::new(name, priority, TaskType::User)
}

/// Get the current task.
/// 
/// # Returns
/// The current task if it exists.
pub fn mytask() -> Option<&'static mut Task> {
    let cpu = get_cpu();
    get_scheduler().get_current_task(cpu.get_cpuid())
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use crate::println;
    use crate::print;

    #[test_case]
    fn test_set_brk() {
        let mut task = super::new_user_task("Task0".to_string(), 0);
        task.init();
        assert_eq!(task.get_brk(), 0);
        task.set_brk(0x1000).unwrap();
        assert_eq!(task.get_brk(), 0x1000);
        task.set_brk(0x2000).unwrap();
        assert_eq!(task.get_brk(), 0x2000);
        task.set_brk(0x1008).unwrap();
        assert_eq!(task.get_brk(), 0x1008);
        task.set_brk(0x1000).unwrap();
        assert_eq!(task.get_brk(), 0x1000);
    }
}