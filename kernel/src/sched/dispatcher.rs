//! Dispatcher module.
//! 
//! The dispatcher module is responsible for dispatching tasks to the CPU.
//! Currently, the dispatcher is a simple dispatcher that runs the task.

use crate::arch::{get_user_trap_handler, set_trapframe, set_trapvector, Arch};
use crate::task::{Task, TaskState, TaskType};
use crate::vm::{get_trampoline_trap_vector, get_trampoline_trapframe};

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
                set_trapvector(get_trampoline_trap_vector());
                set_trapframe(get_trampoline_trapframe(cpu.get_cpuid()));
                cpu.get_trapframe().set_trap_handler(get_user_trap_handler());
                task.vcpu.jump(cpu, task.entry as u64);
            }
            TaskState::Running => {
                set_trapvector(get_trampoline_trap_vector());
                set_trapframe(get_trampoline_trapframe(cpu.get_cpuid()));
                cpu.get_trapframe().set_trap_handler(get_user_trap_handler());
                task.vcpu.switch(cpu);
            }
            TaskState::Terminated => {
            }
            _ => {}
        }
    }
}