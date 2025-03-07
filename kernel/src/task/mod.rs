//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

// pub mod kernel;
// pub mod user;

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

use crate::arch::vcpu::Vcpu;

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
}

static TASK_ID: Mutex<usize> = Mutex::new(0);

impl Task {
    pub fn new(name: String, priority: u32, task_type: TaskType) -> Self {
        let mut taskid = TASK_ID.lock();
        let task = Task { id: *taskid, name, priority, vcpu: Vcpu::new(), state: TaskState::NotInitialized, task_type, entry: 0 };
        *taskid += 1;
        task
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