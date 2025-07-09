//! Scheduler module
//! 
//! The scheduler module is responsible for scheduling tasks on the CPU.
//! Currently, the scheduler is a simple round-robin scheduler with separate
//! queues for different task states to improve efficiency:
//! 
//! - `ready_queue`: Tasks that are ready to run
//! - `blocked_queue`: Tasks waiting for I/O or other events  
//! - `zombie_queue`: Finished tasks waiting to be cleaned up
//! 
//! This separation avoids unnecessary iteration over blocked/zombie tasks
//! during normal scheduling operations.

extern crate alloc;

use alloc::{collections::vec_deque::VecDeque, string::ToString};

use crate::{arch::{enable_interrupt, get_cpu, get_user_trap_handler, instruction::idle, set_trapframe, set_trapvector, trap::user::arch_switch_to_user_space, Arch}, environment::NUM_OF_CPUS, task::{new_kernel_task, wake_task_waiters, TaskState}, timer::get_kernel_timer, vm::{get_kernel_vm_manager, get_trampoline_trap_vector, get_trampoline_trapframe}};
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
    /// Queue for ready-to-run tasks
    ready_queue: [VecDeque<Task>; NUM_OF_CPUS],
    /// Queue for blocked tasks (waiting for I/O, etc.)
    blocked_queue: [VecDeque<Task>; NUM_OF_CPUS],
    /// Queue for zombie tasks (finished but not yet cleaned up)
    zombie_queue: [VecDeque<Task>; NUM_OF_CPUS],
    dispatcher: [Dispatcher; NUM_OF_CPUS],
    interval: u64, /* in microseconds */
    current_task_id: [Option<usize>; NUM_OF_CPUS],
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            ready_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            blocked_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            zombie_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            dispatcher: [const { Dispatcher::new() }; NUM_OF_CPUS],
            interval: 10000, /* 1ms */
            current_task_id: [const { None }; NUM_OF_CPUS],
        }
    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        // Add new tasks to the ready queue by default
        self.ready_queue[cpu_id].push_back(task);
    }

    fn run(&mut self, cpu: &mut Arch) {
        let cpu_id = cpu.get_cpuid();

        // Continue trying to run tasks until we successfully dispatch one or run out of ready tasks
        loop {
            let task = self.ready_queue[cpu_id].pop_front();

            /* If there are no subsequent tasks */
            if self.ready_queue[cpu_id].is_empty() {
                match task {
                    Some(mut t) => {
                        match t.state {
                            TaskState::Zombie => {
                                let task_id = t.get_id();
                                self.zombie_queue[cpu_id].push_back(t);
                                // crate::println!("Scheduler: Task {} is now a zombie", task_id);
                                self.current_task_id[cpu_id] = None;
                                // Wake up any processes waiting for this task
                                wake_task_waiters(task_id);
                                continue;
                                // panic!("At least one task must be scheduled");
                            },
                            TaskState::Terminated => {
                                panic!("At least one task must be scheduled");
                            },
                            TaskState::Blocked(_) => {
                                // Reset current_task_id since this task is no longer current
                                if self.current_task_id[cpu_id] == Some(t.get_id()) {
                                    self.current_task_id[cpu_id] = None;
                                }
                                // Put blocked task to blocked queue without running it
                                self.blocked_queue[cpu_id].push_back(t);
                                continue;
                            },
                            _ => {
                                if self.current_task_id[cpu_id].is_none() {
                                    self.dispatcher[cpu_id].dispatch(cpu, &mut t, None);
                                }
                                self.current_task_id[cpu_id] = Some(t.get_id());
                                self.ready_queue[cpu_id].push_back(t);
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
                                let task_id = t.get_id();
                                self.zombie_queue[cpu_id].push_back(t);
                                // Wake up any processes waiting for this task
                                // crate::println!("Scheduler[not empty]: Task {} is now a zombie", task_id);
                                wake_task_waiters(task_id);
                                continue;
                            },
                            TaskState::Terminated => {
                                continue;
                            },
                            TaskState::Blocked(_) => {
                                // Reset current_task_id since this task is no longer current
                                if self.current_task_id[cpu_id] == Some(t.get_id()) {
                                    self.current_task_id[cpu_id] = None;
                                }
                                // Put blocked task back to the end of queue without running it
                                self.blocked_queue[cpu_id].push_back(t);
                                continue;
                            },
                            _ => {
                                let prev_task = match self.current_task_id[cpu_id] {
                                    Some(task_id) => self.ready_queue[cpu_id].iter_mut().find(|t| t.get_id() == task_id),
                                    None => None
                                };
                                if prev_task.is_some() {
                                    self.dispatcher[cpu_id].dispatch(cpu, &mut t, prev_task);
                                }
                                self.current_task_id[cpu_id] = Some(t.get_id());
                                self.ready_queue[cpu_id].push_back(t);
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

        if !self.ready_queue[cpu_id].is_empty() {
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
            Some(task_id) => self.ready_queue[cpu_id].iter_mut().find(|t| t.get_id() == task_id),
            None => None
        }
    }

    /// Returns a mutable reference to the task with the specified ID, if found.
    /// 
    /// This method searches across all task queues (ready, blocked, zombie) to find
    /// the task with the specified ID. This is needed for Waker integration.
    /// 
    /// # Arguments
    /// * `task_id` - The ID of the task to search for.
    /// 
    /// # Returns
    /// A mutable reference to the task if found, or None otherwise.
    pub fn get_task_by_id(&mut self, task_id: usize) -> Option<&mut Task> {
        // Search in ready queues
        for ready_queue in self.ready_queue.iter_mut() {
            if let Some(task) = ready_queue.iter_mut().find(|t| t.get_id() == task_id) {
                return Some(task);
            }
        }
        
        // Search in blocked queues
        for blocked_queue in self.blocked_queue.iter_mut() {
            if let Some(task) = blocked_queue.iter_mut().find(|t| t.get_id() == task_id) {
                return Some(task);
            }
        }
        
        // Search in zombie queues
        for zombie_queue in self.zombie_queue.iter_mut() {
            if let Some(task) = zombie_queue.iter_mut().find(|t| t.get_id() == task_id) {
                return Some(task);
            }
        }
        
        None
    }

    /// Move a task from blocked queue to ready queue when it's woken up
    /// 
    /// This method is called by Waker when a blocked task needs to be woken up.
    /// 
    /// # Arguments
    /// * `task_id` - The ID of the task to move to ready queue
    /// 
    /// # Returns
    /// true if the task was found and moved, false otherwise
    pub fn wake_task(&mut self, task_id: usize) -> bool {
        crate::println!("Scheduler: Waking up task {}", task_id);
        // Search for the task in blocked queues
        for cpu_id in 0..self.blocked_queue.len() {
            if let Some(pos) = self.blocked_queue[cpu_id].iter().position(|t| t.get_id() == task_id) {
                if let Some(mut task) = self.blocked_queue[cpu_id].remove(pos) {
                    // Set task state to Running
                    task.state = TaskState::Running;
                    // crate::println!("Scheduler: Task {} waking up with PC: 0x{:x}", task_id, task.vcpu.get_pc());
                    // Move to ready queue
                    self.ready_queue[cpu_id].push_back(task);
                    // crate::println!("Scheduler: Woke up task {} and moved it to ready queue", task_id);
                    return true;
                }
            }
        }
        false
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
        assert_eq!(scheduler.ready_queue[0].len(), 1);
    }
}