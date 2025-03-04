//! Dispatcher module.
//! 
//! The dispatcher module is responsible for dispatching tasks to the CPU.
//! Currently, the dispatcher is a simple dispatcher that runs the task.

use crate::arch::Arch;
use crate::task::{Task, TaskState, TaskType};

pub struct Dispatcher {
    pub current_task: Option<usize>,
}

impl Dispatcher {
    pub const fn new() -> Self {
        Dispatcher { current_task: None }
    }

    pub fn dispatch(&mut self, cpu: &mut Arch, task: &mut Task) {
        match task.state {
            TaskState::NotInitialized => {
                match task.task_type {
                    TaskType::Kernel => {
                        task.state = TaskState::Ready;
                    }
                    TaskType::User => {
                        task.state = TaskState::Ready;
                    }
                }
            }
            TaskState::Ready => {
                task.state = TaskState::Running;
                let id = task.get_id();
                self.current_task = Some(id);
                task.vcpu.jump(cpu, 0x00);
            }
            TaskState::Running => {
                let id = task.get_id();
                self.current_task = Some(id);
                task.vcpu.switch(cpu);
            }
            TaskState::Terminated => {
            }
            _ => {}
        }
    }
}