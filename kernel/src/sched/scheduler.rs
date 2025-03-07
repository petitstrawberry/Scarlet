//! Scheduler module
//! 
//! The scheduler module is responsible for scheduling tasks on the CPU.
//! Currently, the scheduler is a simple round-robin scheduler.

extern crate alloc;

use alloc::collections::vec_deque::VecDeque;
use alloc::string::String;

use crate::{arch::{enable_interrupt, get_cpu, instruction::idle, get_user_trap_handler, set_trapvector, Arch}, environment::NUM_OF_CPUS, task::{new_kernel_task, TaskState}, timer::get_kernel_timer, vm::{get_trampoline_trap_vector, get_trampoline_trapframe, set_trampoline_trapframe}};
use crate::println;
use crate::print;

use super::dispatcher::Dispatcher;
use crate::task::Task;

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
    current_task: Option<usize>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            task_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            dispatcher: [const { Dispatcher::new() }; NUM_OF_CPUS],
            interval: 1000, /* 1ms */
            current_task: None,
        }
    }

    pub fn init_test_tasks(&mut self) {
        let task0 = new_kernel_task(String::from("Task0"), 0, || {
            println!("Task0");
            let mut counter = 0;
            loop {
                // println!("Task0: {}", counter);
                // counter += 1;
            }
        });
        self.add_task(task0, 0);

        let task1 = new_kernel_task(String::from("Task1"), 0, || {
            println!("Task1");
            // let mut counter = 0;
            loop {
                // println!("Task1: {}", counter);
                // counter += 1;
            }
        });

        self.add_task(task1, 0);


    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        self.task_queue[cpu_id].push_back(task);
    }

    fn run(&mut self, cpu: &mut Arch) {
        let cpu_id = cpu.get_cpuid();

        if let Some(mut t) = self.task_queue[cpu_id].pop_front() {

            // currentt_taskのidxに対応するtaskを取得
            let prev_task = match self.current_task {
                Some(id) => {
                    let idx = self.task_queue[cpu_id].iter().position(|t| t.get_id() == id);
                    match idx {
                        Some(i) => Some(self.task_queue[cpu_id].get_mut(i).unwrap()),
                        None => None,
                    }
                },
                None => None,
            };

            self.dispatcher[cpu_id].dispatch(cpu, &mut t, prev_task);

            self.current_task = Some(t.get_id());
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

    /* MUST NOT raise any exception in this function before the idle loop */
    pub fn start_scheduler(&mut self) {
        let cpu = get_cpu();
        let cpu_id = cpu.get_cpuid();
        let timer = get_kernel_timer();
        timer.stop(cpu_id);

        let trap_vector = get_trampoline_trap_vector();
        let trapframe = get_trampoline_trapframe(cpu_id);
        set_trapvector(trap_vector);
        set_trampoline_trapframe(cpu_id, trapframe);
        cpu.get_trapframe().set_trap_handler(get_user_trap_handler());

        /* Jump to trap handler immediately */
        timer.set_interval_us(cpu_id, 0);
        enable_interrupt();
        
        // kernel_vm_switch(); /* After this point, the kernel is running in virtual memory */
        if !self.task_queue[cpu_id].is_empty() {
            timer.start(cpu_id);
        }
        idle(); /* idle loop */
    }
}
#[cfg(test)]
mod tests {
    use crate::task::TaskType;

    use super::*;

    #[test_case]
    fn test_add_task() {
        let mut scheduler = Scheduler::new();
        let task = Task::new(String::from("TestTask"), 1, TaskType::Kernel);
        scheduler.add_task(task, 0);
        assert_eq!(scheduler.task_queue[0].len(), 1);
    }
}
