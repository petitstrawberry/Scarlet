//! Scheduler module
//! 
//! The scheduler module is responsible for scheduling tasks on the CPU.
//! Currently, the scheduler is a simple round-robin scheduler.

extern crate alloc;

use alloc::vec::Vec;

use crate::arch::instruction::idle;

use super::{dispatcher::Dispatcher, task::Task};

const NUM_OF_CPUS: usize = 8;

pub struct Scheduler {
    task_queue: [Vec<Task>; NUM_OF_CPUS],
    dispatcher: [Dispatcher; NUM_OF_CPUS],
}

impl Scheduler {
    pub fn new() -> Self {
        Scheduler {
            task_queue: [const { Vec::new() }; NUM_OF_CPUS],
            dispatcher: [const { Dispatcher::new() }; NUM_OF_CPUS],
        }
    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        self.task_queue[cpu_id].push(task);
    }

    pub fn run(&mut self) {
        let cpu_id = 0;
        let task = self.task_queue[cpu_id].pop();
        match task {
            Some(t) => self.dispatcher[cpu_id].dispatch(&t),
            None => {}
        }
    }

    pub fn schedule(&mut self) {
        let cpu_id = 0;
        
        while !self.task_queue[cpu_id].is_empty() {
            self.run();
        }
        idle();
    }
}