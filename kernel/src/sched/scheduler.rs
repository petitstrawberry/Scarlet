//! Scheduler module
//! 
//! The scheduler module is responsible for scheduling tasks on the CPU.
//! Currently, the scheduler is a simple round-robin scheduler.

extern crate alloc;

use alloc::collections::vec_deque::VecDeque;
use alloc::string::String;

use crate::{arch::{enable_interrupt, instruction::idle, Arch}, environment::NUM_OF_CPUS, library::syscall::schedule, task::TaskState, timer::get_kernel_timer};

use super::dispatcher::Dispatcher;
use crate::task::{Task, TaskType};



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
    task_queue: [VecDeque<Task>; NUM_OF_CPUS],
    dispatcher: [Dispatcher; NUM_OF_CPUS],
    interval: u64, /* in microseconds */
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            task_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            dispatcher: [const { Dispatcher::new() }; NUM_OF_CPUS],
            interval: 1000, /* 1ms */
        }
    }

    pub fn init_test_tasks(&mut self) {
        let task1 = Task::new(String::from("Task1"), 1, TaskType::Kernel);
        let task2 = Task::new(String::from("Task2"), 2, TaskType::Kernel);
        let task3 = Task::new(String::from("Task3"), 3, TaskType::Kernel);

        self.add_task(task1, 0);
        self.add_task(task2, 0);
        self.add_task(task3, 0);
    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        self.task_queue[cpu_id].push_back(task);
    }

    fn run(&mut self, cpu: &mut Arch) {
        let cpu_id = 0;
        if let Some(mut t) = self.task_queue[cpu_id].pop_front() {
            self.dispatcher[cpu_id].dispatch(cpu, &mut t);
            if t.state != TaskState::Terminated {
                self.task_queue[cpu_id].push_back(t);
            }
        }
    }

    pub fn schedule(&mut self, cpu: &mut Arch) {
        let cpu_id = cpu.get_cpuid();

        let timer = get_kernel_timer();
        timer.stop(cpu_id);
        timer.set_interval_us(cpu_id, self.interval);

        if !self.task_queue[cpu_id].is_empty() {
            timer.start(cpu_id);
            self.run(cpu);
        }
    }

    pub fn kernel_schedule(&mut self, cpu_id: usize) {
        let timer = get_kernel_timer();
        timer.stop(cpu_id);
        /* Jump to trap handler immediately */
        timer.set_interval_us(cpu_id, 0);
        enable_interrupt();
        
        if !self.task_queue[cpu_id].is_empty() {
            timer.start(cpu_id);
        }
        idle();
    }
}