//! Scheduler module
//! 
//! The scheduler module is responsible for scheduling tasks on the CPU.
//! Currently, the scheduler is a simple round-robin scheduler.

extern crate alloc;

use core::panic;

use alloc::{collections::vec_deque::VecDeque, string::ToString};

use crate::{arch::{enable_interrupt, get_cpu, get_user_trap_handler, instruction::idle, set_trapframe, set_trapvector, trap::user::arch_switch_to_user_space, Arch}, environment::NUM_OF_CPUS, task::{new_kernel_task, TaskState}, timer::get_kernel_timer, vm::{get_kernel_vm_manager, get_trampoline_trap_vector, get_trampoline_trapframe}};
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
    add_req_queue: [VecDeque<Task>; NUM_OF_CPUS],
    dispatcher: [Dispatcher; NUM_OF_CPUS],
    interval: u64, /* in microseconds */
    current_task_id: [Option<usize>; NUM_OF_CPUS],
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            task_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            add_req_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            dispatcher: [const { Dispatcher::new() }; NUM_OF_CPUS],
            interval: 10000, /* 1ms */
            current_task_id: [const { None }; NUM_OF_CPUS],
        }
    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        self.task_queue[cpu_id].push_back(task);
    }

    fn run(&mut self, cpu: &mut Arch) {
        let cpu_id = cpu.get_cpuid();

        if !self.add_req_queue[cpu_id].is_empty() {
            for task in self.add_req_queue[cpu_id].drain(..) {
                self.task_queue[cpu_id].push_back(task);
            }
        }

        loop {
            let task = self.task_queue[cpu_id].pop_front();

            /* If there are no subsequent tasks */
            if self.task_queue[cpu_id].is_empty() {
                match task {
                    Some(mut t) => {
                        match t.state {
                            TaskState::Zombie => {
                                panic!("At least one task must be scheduled");
                            },
                            TaskState::Terminated => {
                                panic!("At least one task must be scheduled");
                            },
                            _ => {
                                if self.current_task_id[cpu_id].is_none() {
                                    self.dispatcher[cpu_id].dispatch(cpu, &mut t, None);
                                }
                                self.current_task_id[cpu_id] = Some(t.get_id());
                                self.task_queue[cpu_id].push_back(t);
                                break;
                            }
                        }
                    }
                    /* If the task queue is empty, run the same task again */
                    None => break,
                }
            } else {
                match task {
                    Some(mut t) => {
                        match t.state {
                            TaskState::Zombie => {
                                self.task_queue[cpu_id].push_back(t);
                                continue;
                            },
                            TaskState::Terminated => {
                                continue;
                            },
                            _ => {
                                let prev_task = match self.current_task_id[cpu_id] {
                                    Some(task_id) => self.task_queue[cpu_id].iter_mut().find(|t| t.get_id() == task_id),
                                    None => None
                                };
                                if prev_task.is_some() {
                                    self.dispatcher[cpu_id].dispatch(cpu, &mut t, prev_task);
                                }
                                self.current_task_id[cpu_id] = Some(t.get_id());
                                self.task_queue[cpu_id].push_back(t);
                                break;
                            }
                        }
                    }
                    None => break,
                }
            }
        }
    }

    // Schedule tasks on the CPU
    // This function is called by the timer interrupt handler.
    // It runs the current task and switches to user space if there are tasks in the queue
    // or goes to idle if the queue is empty.
    // This function should not raise any exceptions before the idle loop.
    // It is expected to be called from the timer interrupt handler.
    pub fn schedule(&mut self, cpu: &mut Arch) -> ! {
        let cpu_id = cpu.get_cpuid();

        let timer = get_kernel_timer();
        timer.stop(cpu_id);
        timer.set_interval_us(cpu_id, self.interval);

        if !self.task_queue[cpu_id].is_empty() {
            self.run(cpu);
            timer.start(cpu_id);
            arch_switch_to_user_space(cpu);
        }
        // If the task queue is empty, go to idle
        timer.start(cpu_id);
        idle();
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
        set_trapframe(trapframe);
        // cpu.get_trapframe().set_trap_handler(get_user_trap_handler());
        let trapframe = cpu.get_trapframe();
        trapframe.set_trap_handler(get_user_trap_handler());
        trapframe.set_next_address_space(get_kernel_vm_manager().get_asid());

        /* Jump to trap handler immediately */
        timer.set_interval_us(cpu_id, 0);
        enable_interrupt();
        timer.start(cpu_id);
        idle();
    }

    pub fn get_current_task(&mut self, cpu_id: usize) -> Option<&mut Task> {
        match self.current_task_id[cpu_id] {
            Some(task_id) => self.task_queue[cpu_id].iter_mut().find(|t| t.get_id() == task_id),
            None => None
        }
    }

    /// Returns a mutable reference to the task with the specified ID, if found.
    /// 
    /// # Arguments
    /// * `task_id` - The ID of the task to search for.
    /// 
    /// # Returns
    /// A mutable reference to the task if found, or None otherwise.
    pub fn get_task_by_id(&mut self, task_id: usize) -> Option<&mut Task> {
        for task_queue in self.task_queue.iter_mut() {
            if let Some(task) = task_queue.iter_mut().find(|t| t.get_id() == task_id) {
                return Some(task);
            }
        }
        None
    }
}

pub fn make_test_tasks() {
    println!("Making test tasks...");
    let sched = get_scheduler();
    let mut task0 = new_kernel_task("Task0".to_string(), 0, || {
        println!("Task0");
        let mut counter: usize = 0;
        loop {
            if counter % 500000 == 0 {
                print!("\nTask0: ");
            }
            if counter % 10000 == 0 {
                print!(".");
            }
            counter += 1;
            if counter >= 100000000 {
                break;
            }
        }
        println!("");
        println!("Task0: Done");
        idle();
    });
    task0.init();
    sched.add_task(task0, 0);

    let mut task1 = new_kernel_task("Task1".to_string(), 0, || {
        println!("Task1");
        let mut counter: usize = 0;
        loop {
            if counter % 500000 == 0 {
                print!("\nTask1: {} %", counter / 1000000);
            }
            counter += 1;
            if counter >= 100000000 {
                break;
            }
        }
        println!("\nTask1: 100 %");
        println!("Task1: Completed");
        idle();
    });
    task1.init();
    sched.add_task(task1, 0);

    let mut task2 = new_kernel_task("Task2".to_string(), 0, || {
        println!("Task2");
        /* Fizz Buzz */
        for i in 1..=1000000 {
            if i % 1000 > 0 {
                continue;
            }
            let c = i / 1000;
            if c % 15 == 0 {
                println!("FizzBuzz");
            } else if c % 3 == 0 {
                println!("Fizz");
            } else if c % 5 == 0 {
                println!("Buzz");
            } else {
                println!("{}", c);
            }
        }
        println!("Task2: Done");
        idle();
    });
    task2.init();
    sched.add_task(task2, 0);
}

// late_initcall!(make_test_tasks);

#[cfg(test)]
mod tests {
    use crate::task::TaskType;

    use super::*;

    #[test_case]
    fn test_add_task() {
        let mut scheduler = Scheduler::new();
        let task = Task::new("TestTask".to_string(), 1, TaskType::Kernel);
        scheduler.add_task(task, 0);
        assert_eq!(scheduler.task_queue[0].len(), 1);
    }
}