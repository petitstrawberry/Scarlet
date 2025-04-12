//! Dispatcher module.
//! 
//! The dispatcher module is responsible for dispatching tasks to the CPU.
//! Currently, the dispatcher is a simple dispatcher that runs the task.

use crate::arch::{get_user_trap_handler, set_next_mode, set_trapvector, Arch};

use crate::task::{Task, TaskState, TaskType};
use crate::vm::get_trampoline_trap_vector;

pub struct Dispatcher;

impl Dispatcher {
    pub const fn new() -> Self {
        Dispatcher {}
    }

    #[allow(static_mut_refs)]
    pub fn dispatch(&mut self, cpu: &mut Arch, task: &mut Task, prev_task: Option<&mut Task>) {
        if let Some(prev_task) = prev_task {
            prev_task.vcpu.store(cpu);
        }

        match task.state {
            TaskState::NotInitialized => {
                match task.task_type {
                    TaskType::Kernel => {
                        panic!("Kernel task should not be in NotInitialized state");
                    }
                    TaskType::User => {
                        panic!("User task should not be in NotInitialized state");
                    }
                }
            }
            TaskState::Ready => {
                task.state = TaskState::Running;
                set_trapvector(get_trampoline_trap_vector());
                let trapframe = cpu.get_trapframe();
                trapframe.set_trap_handler(get_user_trap_handler());
                trapframe.set_next_address_space(task.vm_manager.get_asid());
                task.vcpu.set_pc(task.entry as u64);
                task.vcpu.switch(cpu);
                set_next_mode(task.vcpu.get_mode());
            }
            TaskState::Running => {
                set_trapvector(get_trampoline_trap_vector());
                let trapframe = cpu.get_trapframe();
                trapframe.set_trap_handler(get_user_trap_handler());
                trapframe.set_next_address_space(task.vm_manager.get_asid());
                task.vcpu.switch(cpu);
                set_next_mode(task.vcpu.get_mode());
            }
            TaskState::Terminated => {
            }
            _ => {}
        }
    }
}