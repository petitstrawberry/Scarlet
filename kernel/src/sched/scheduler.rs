//! Scheduler module
//! 
//! The scheduler module is responsible for scheduling tasks on the CPU.
//! Currently, the scheduler is a simple round-robin scheduler.

extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;

use crate::{arch::instruction::idle, environment::NUM_OF_CPUS, timer::get_kernel_timer};

use super::{dispatcher::Dispatcher, task::Task};



static mut SCHEDULER: Option<Scheduler> = None;

pub fn get_scheduler() -> &'static mut Scheduler {
    unsafe {
        match SCHEDULER {
            Some(ref mut s) => s,
            None => {
                SCHEDULER = Some(Scheduler::new());
                get_scheduler()
            }
        }
    }
}

pub struct Scheduler {
    task_queue: [Vec<Task>; NUM_OF_CPUS],
    dispatcher: [Dispatcher; NUM_OF_CPUS],
    interval: u64, /* in microseconds */
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            task_queue: [const { Vec::new() }; NUM_OF_CPUS],
            dispatcher: [const { Dispatcher::new() }; NUM_OF_CPUS],
            interval: 1000, /* 1ms */
        }
    }

        }
    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        self.task_queue[cpu_id].push(task);
    }

    fn run(&mut self) {
        let cpu_id = 0;
        let task = self.task_queue[cpu_id].pop();
        match task {
            Some(t) => self.dispatcher[cpu_id].dispatch(&t),
            None => {}
        }
    }

    pub fn schedule(&mut self) {
        let cpu_id = 0;

        let timer = get_kernel_timer();
        timer.stop(cpu_id);
        timer.set_interval_us(cpu_id, self.interval);
        enable_interrupt();

        if !self.task_queue[cpu_id].is_empty() {
            timer.start(cpu_id);
            self.run();
        }
        idle();
    }
}