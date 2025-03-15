//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

pub mod syscall;

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

use crate::{arch::{get_cpu, vcpu::Vcpu}, environment::{DEAFAULT_MAX_TASK_DATA_SIZE, DEAFAULT_MAX_TASK_STACK_SIZE, DEAFAULT_MAX_TASK_TEXT_SIZE, KERNEL_VM_STACK_END, PAGE_SIZE}, mem::page::{allocate_pages, free_pages, Page}, sched::scheduler::get_scheduler, vm::{manager::VirtualMemoryManager, user_kernel_vm_init, user_vm_init, vmem::{MemoryArea, VirtualMemoryMap, VirtualMemorySegment}}};
use crate::println;
use crate::print;

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

    /* Get total size of allocated memory */
    pub fn get_size(&self) -> usize {
        self.stack_size + self.text_size + self.data_size
    }

    /* Get the program break */
    pub fn get_brk(&self) -> usize {
        self.text_size + self.data_size
    }

    /* Set the program break */
    pub fn set_brk(&mut self, brk: usize) -> Result<(), &'static str> {
        if brk < self.text_size {
            return Err("Invalid address");
        }
        let prev_brk = self.get_brk();
        let prev_page = prev_brk / PAGE_SIZE;
        let page = brk / PAGE_SIZE;

        if page < prev_page {
            /* Free pages */
            let num_of_pages = prev_page - page;
            self.free_pages(page * PAGE_SIZE, num_of_pages);
            
        } else if page > prev_page {
            /* Allocate pages */
            let num_of_pages = page - prev_page;
            match self.allocate_pages(page * PAGE_SIZE, num_of_pages, VirtualMemorySegment::Data) {
                Ok(mmap) => {
                    println!("Allocated pages {}", mmap.vmarea.start);
                },
                Err(_) => return Err("Failed to allocate pages"),
            }
        } /* If page == prev_page, do nothing */

        self.data_size = brk - self.text_size;
        Ok(())
    }

    /* Allocate a page for the task */
    pub fn allocate_pages(&mut self, vaddr: usize, num_of_pages: usize, segment: VirtualMemorySegment) -> Result<VirtualMemoryMap, &'static str> {
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
        Ok(mmap)
    }

    pub fn free_pages(&mut self, vaddr: usize, num_of_pages: usize) {
        let page = vaddr / PAGE_SIZE;
        for p in 0..num_of_pages {
            let vaddr = (page + p) * PAGE_SIZE;
            match self.vm_manager.search_memory_map_idx(vaddr) {
                Some(idx) => {
                    self.vm_manager.remove_memory_map(idx);
                },
                None => {},
            }
        }
        free_pages((page * PAGE_SIZE) as *mut Page, num_of_pages);
        /* Unmap pages */
        let root_pagetable = self.vm_manager.get_root_page_table().unwrap();
        for p in 0..num_of_pages {
            let vaddr = (page + p) * PAGE_SIZE;
            root_pagetable.unmap(vaddr);
        }
    }
}


pub fn new_kernel_task(name: String, priority: u32, func: fn()) -> Task {
    let mut task = Task::new(name, priority, TaskType::Kernel);
    task.entry = func as usize;
    task
}

pub fn new_user_task(name: String, priority: u32) -> Task {
    Task::new(name, priority, TaskType::User)
}

pub fn mytask() -> Option<&'static mut Task> {
    let cpu = get_cpu();
    get_scheduler().get_current_task(cpu.get_cpuid())
}