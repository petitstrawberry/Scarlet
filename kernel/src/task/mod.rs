//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

// pub mod kernel;
// pub mod user;

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

use crate::{arch::vcpu::Vcpu, environment::KERNEL_VM_STACK_END, vm::{manager::VirtualMemoryManager, user_kernel_vm_init, user_vm_init}};

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
    pub size: usize, /* Size of the allocated memory for the task */
    pub vm_manager: VirtualMemoryManager,
}

static TASK_ID: Mutex<usize> = Mutex::new(0);

impl Task {
    pub fn new(name: String, priority: u32, task_type: TaskType) -> Self {
        let mut taskid = TASK_ID.lock();
        let task = Task { id: *taskid, name, priority, vcpu: Vcpu::new(), state: TaskState::NotInitialized, task_type, entry: 0, size: 0, vm_manager: VirtualMemoryManager::new() };
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
}


pub fn new_kernel_task(name: String, priority: u32, func: fn()) -> Task {
    let mut task = Task::new(name, priority, TaskType::Kernel);
    task.entry = func as usize;
    task
}

pub fn new_user_task(name: String, priority: u32) -> Task {
    Task::new(name, priority, TaskType::User)
}