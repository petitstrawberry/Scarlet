//! Dispatcher module.
//! 
//! The dispatcher module is responsible for dispatching tasks to the CPU.
//! Currently, the dispatcher is a simple dispatcher that runs the task.

use crate::arch::Arch;
use crate::task::{Task, TaskState, TaskType};

pub struct Dispatcher;

impl Dispatcher {
    pub const fn new() -> Self {
        Dispatcher {}
    }

    pub fn dispatch(&mut self, cpu: &mut Arch, task: &mut Task, prev_task: Option<&mut Task>) {

        if let Some(prev_task) = prev_task {
            prev_task.vcpu.store(cpu);
        }

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
                task.vcpu.jump(cpu, task.entry as u64);
            }
            TaskState::Running => {
                task.vcpu.switch(cpu);
            }
            TaskState::Terminated => {
            }
            _ => {}
        }
    }
}