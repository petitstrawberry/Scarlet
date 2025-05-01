//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

pub mod syscall;
pub mod elf_loader;

extern crate alloc;

use alloc::{string::String, vec::Vec};
use spin::Mutex;

use crate::{arch::{get_cpu, vcpu::Vcpu}, environment::{DEAFAULT_MAX_TASK_DATA_SIZE, DEAFAULT_MAX_TASK_STACK_SIZE, DEAFAULT_MAX_TASK_TEXT_SIZE, KERNEL_VM_STACK_END, PAGE_SIZE}, mem::page::{allocate_raw_pages, free_raw_pages, Page}, sched::scheduler::get_scheduler, vm::{manager::VirtualMemoryManager, user_kernel_vm_init, user_vm_init, vmem::{MemoryArea, VirtualMemoryMap, VirtualMemorySegment}}};


#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TaskState {
    NotInitialized,
    Ready,
    Running,
    Blocked,
    Zombie,
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
    parent_id: Option<usize>,      /* Parent task ID */
    children: Vec<usize>,          /* List of child task IDs */
    exit_status: Option<i32>,      /* Exit code (for monitoring child task termination) */
}

static TASK_ID: Mutex<usize> = Mutex::new(1);

impl Task {
    pub fn new(name: String, priority: u32, task_type: TaskType) -> Self {
        let mut taskid = TASK_ID.lock();
        let task = Task {
            id: *taskid,
            name,
            priority,
            vcpu: Vcpu::new(match task_type {
                TaskType::Kernel => crate::arch::vcpu::Mode::Kernel,
                TaskType::User => crate::arch::vcpu::Mode::User,
            }),
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
            parent_id: None,
            children: Vec::new(),
            exit_status: None,
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
            TaskType::User => { 
                user_vm_init(self);
                /* Set sp to the top of the user stack */
                self.vcpu.regs.reg[2] = 0xffff_ffff_ffff_f000;
            }
        }
        
        /* Set the task state to Ready */
        self.state = TaskState::Ready;
    }

    pub fn get_id(&self) -> usize {
        self.id
    }

    /// Set the task state
    /// 
    /// # Arguments
    /// * `state` - The new task state
    /// 
    pub fn set_state(&mut self, state: TaskState) {
        self.state = state;
    }

    /// Get the task state
    /// 
    /// # Returns
    /// The task state
    /// 
    pub fn get_state(&self) -> TaskState {
        self.state
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
        let pages = allocate_raw_pages(num_of_pages);
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
        self.vm_manager.add_memory_map(mmap).map_err(|e| panic!("Failed to add memory map: {}", e))?;
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
                        self.vm_manager.add_memory_map(mmap1)
                            .map_err(|e| panic!("Failed to add memory map: {}", e)).unwrap();
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
                        self.vm_manager.add_memory_map(mmap2)
                            .map_err(|e| panic!("Failed to add memory map: {}", e)).unwrap();
                        // println!("Removed map : {:#x} - {:#x}", mmap.vmarea.start, mmap.vmarea.end);
                        // println!("Re added map: {:#x} - {:#x}", mmap2.vmarea.start, mmap2.vmarea.end);
                    }
                    let offset = vaddr - mmap.vmarea.start;
                    free_raw_pages((mmap.pmarea.start + offset) as *mut Page, 1);
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

    /// Get the parent ID
    ///
    /// # Returns
    /// The parent task ID, or None if there is no parent
    pub fn get_parent_id(&self) -> Option<usize> {
        self.parent_id
    }
    
    /// Set the parent task
    ///
    /// # Arguments
    /// * `parent_id` - The ID of the parent task
    pub fn set_parent_id(&mut self, parent_id: usize) {
        self.parent_id = Some(parent_id);
    }
    
    /// Add a child task
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task
    pub fn add_child(&mut self, child_id: usize) {
        if !self.children.contains(&child_id) {
            self.children.push(child_id);
        }
    }
    
    /// Remove a child task
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task to remove
    ///
    /// # Returns
    /// true if the removal was successful, false if the child task was not found
    pub fn remove_child(&mut self, child_id: usize) -> bool {
        if let Some(pos) = self.children.iter().position(|&id| id == child_id) {
            self.children.remove(pos);
            true
        } else {
            false
        }
    }
    
    /// Get the list of child tasks
    ///
    /// # Returns
    /// A slice of child task IDs
    pub fn get_children(&self) -> &[usize] {
        &self.children
    }
    
    /// Set the exit status
    ///
    /// # Arguments
    /// * `status` - The exit status
    pub fn set_exit_status(&mut self, status: i32) {
        self.exit_status = Some(status);
    }
    
    /// Get the exit status
    ///
    /// # Returns
    /// The exit status, or None if not set
    pub fn get_exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// Clone this task, creating a near-identical copy
    /// 
    /// # Returns
    /// The cloned task
    /// 
    /// # Errors 
    /// If the task cannot be cloned, an error is returned.
    ///
    pub fn clone_task(&mut self) -> Result<Task, &'static str> {
        // Create a new task
        let mut child = Task::new(
            self.name.clone(),
            self.priority,
            self.task_type
        );
        child.init();
        
        // Copy memory maps
        for mmap in self.vm_manager.get_memmap() {
            // Allocate new pages for each memory region
            let num_pages = (mmap.vmarea.end - mmap.vmarea.start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
            let vaddr = mmap.vmarea.start;
            
            if num_pages > 0 {
                // Create a new memory map
                let permissions = mmap.permissions;
                let pages = allocate_raw_pages(num_pages);
                let size = num_pages * PAGE_SIZE;
                let paddr = pages as usize;
                let new_mmap = VirtualMemoryMap {
                    pmarea: MemoryArea {
                        start: paddr,
                        end: paddr + (size - 1),
                    },
                    vmarea: MemoryArea {
                        start: vaddr,
                        end: vaddr + (size - 1),
                    },
                    permissions,
                };
                
                // Copy the contents of the original memory
                for i in 0..num_pages {
                    let src_page_addr = mmap.pmarea.start + i * PAGE_SIZE;
                    let dst_page_addr = new_mmap.pmarea.start + i * PAGE_SIZE;
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            src_page_addr as *const u8,
                            dst_page_addr as *mut u8,
                            PAGE_SIZE
                        );
                    }
                }
                
                // Add the new memory map to the child task
                child.vm_manager.add_memory_map(new_mmap)
                    .map_err(|_| "Failed to add memory map to child task")?;
            }
        }
        
        // Copy register states
        child.vcpu.regs = self.vcpu.regs.clone();
        
        // Copy state such as data size
        child.stack_size = self.stack_size;
        child.data_size = self.data_size;
        child.text_size = self.text_size;
        child.max_stack_size = self.max_stack_size;
        child.max_data_size = self.max_data_size;
        child.max_text_size = self.max_text_size;
        
        // Set the same entry point and PC
        child.entry = self.entry;
        child.vcpu.set_pc(self.vcpu.get_pc());
        
        // Set the state to Ready
        child.state = self.state;

        // Set parent-child relationship
        child.set_parent_id(self.id);
        self.add_child(child.get_id());

        Ok(child)
    }

    /// Exit the task
    /// 
    /// # Arguments
    /// * `status` - The exit status
    /// 
    pub fn exit(&mut self, status: i32) {
        match self.parent_id {
            Some(parent_id) => {
                if get_scheduler().get_task_by_id(parent_id).is_none() {
                    self.state = TaskState::Terminated;
                    return;
                }
                /* Set the exit status */
                self.set_exit_status(status);
                self.state = TaskState::Zombie;
            },
            None => {
                /* If the task has no parent, it is terminated */
                self.state = TaskState::Terminated;
            }
        }
    }

    /// Wait for a child task to exit and collect its status
    /// 
    /// # Arguments
    /// * `child_id` - The ID of the child task to wait for
    /// 
    /// # Returns
    /// The exit status of the child task, or an error if the child is not found or not in Zombie state
    pub fn wait(&mut self, child_id: usize) -> Result<i32, &'static str> {
        if !self.children.contains(&child_id) {
            return Err("No such child");
        }

        if let Some(child_task) = get_scheduler().get_task_by_id(child_id) {
            if child_task.get_state() == TaskState::Zombie {
                let status = child_task.get_exit_status().unwrap_or(-1);
                child_task.set_state(TaskState::Terminated);
                self.remove_child(child_id);
                Ok(status)
            } else {
                Err("Child has not exited or is not a zombie")
            }
        } else {
            Err("Child task not found")
        }
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

    #[test_case]
    fn test_task_parent_child_relationship() {
        let mut parent_task = super::new_user_task("ParentTask".to_string(), 0);
        parent_task.init();

        let mut child_task = super::new_user_task("ChildTask".to_string(), 0);
        child_task.init();

        // Set parent-child relationship
        child_task.set_parent_id(parent_task.get_id());
        parent_task.add_child(child_task.get_id());

        // Verify parent-child relationship
        assert_eq!(child_task.get_parent_id(), Some(parent_task.get_id()));
        assert!(parent_task.get_children().contains(&child_task.get_id()));

        // Remove child and verify
        assert!(parent_task.remove_child(child_task.get_id()));
        assert!(!parent_task.get_children().contains(&child_task.get_id()));
    }

    #[test_case]
    fn test_task_exit_status() {
        let mut task = super::new_user_task("TaskWithExitStatus".to_string(), 0);
        task.init();

        // Verify initial exit status is None
        assert_eq!(task.get_exit_status(), None);

        // Set and verify exit status
        task.set_exit_status(0);
        assert_eq!(task.get_exit_status(), Some(0));

        task.set_exit_status(1);
        assert_eq!(task.get_exit_status(), Some(1));
    }
}