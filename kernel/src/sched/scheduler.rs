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

use core::{future::Ready, panic};

use alloc::{collections::vec_deque::VecDeque, string::ToString, task};

use crate::{arch::{enable_interrupt, get_cpu, get_user_trap_handler, instruction::idle, interrupt::enable_external_interrupts, kernel, set_next_mode, set_trapframe, set_trapvector, trap::user::arch_switch_to_user_space, Arch}, environment::NUM_OF_CPUS, task::{new_kernel_task, wake_parent_waiters, wake_task_waiters, TaskState}, timer::get_kernel_timer, vm::{get_kernel_vm_manager, get_trampoline_trap_vector, get_trampoline_trapframe}};
use crate::println;
use crate::print;

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
    interval: u64, /* in microseconds */
    current_task_id: [Option<usize>; NUM_OF_CPUS],
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            ready_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            blocked_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            zombie_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            interval: 10000, /* 1ms */
            current_task_id: [const { None }; NUM_OF_CPUS],
        }
    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        // Add new tasks to the ready queue by default
        self.ready_queue[cpu_id].push_back(task);
    }

    /// Determines the next task to run and returns current and next task IDs
    /// 
    /// This method performs the core scheduling algorithm and task state management
    /// without performing actual context switches or hardware setup.
    /// 
    /// # Arguments
    /// * `cpu` - The CPU architecture state (for CPU ID)
    /// 
    /// # Returns
    /// * `(old_task_id, new_task_id)` - Tuple of old and new task IDs
    fn run(&mut self, cpu: &mut Arch) -> (Option<usize>, Option<usize>) {
        let cpu_id = cpu.get_cpuid();
        let old_current_task_id = self.current_task_id[cpu_id];

        // Continue trying to find a suitable task to run
        loop {
            let task = self.ready_queue[cpu_id].pop_front();

            /* If there are no subsequent tasks */
            if self.ready_queue[cpu_id].is_empty() {
                match task {
                    Some(mut t) => {
                        match t.state {
                            TaskState::NotInitialized => {
                                panic!("Task must be initialized before scheduling");
                            },
                            TaskState::Zombie => {
                                let task_id = t.get_id();
                                let parent_id = t.get_parent_id();
                                self.zombie_queue[cpu_id].push_back(t);
                                self.current_task_id[cpu_id] = None;
                                // Wake up any processes waiting for this specific task
                                wake_task_waiters(task_id);
                                // Also wake up parent process for waitpid(-1)
                                if let Some(parent_id) = parent_id {
                                    wake_parent_waiters(parent_id);
                                }
                                continue;
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
                            TaskState::Ready | TaskState::Running => {
                                t.state = TaskState::Running;
                                // Task is ready to run
                                t.time_slice = 1; // Reset time slice on dispatch
                                let next_task_id = t.get_id();
                                self.current_task_id[cpu_id] = Some(next_task_id);
                                self.ready_queue[cpu_id].push_back(t);
                                return (old_current_task_id, Some(next_task_id));
                            }
                        }
                    }
                    // If no tasks are ready, create an idle task
                    None => {
                        let mut kernel_task = new_kernel_task("idle".to_string(), 0, || {
                            // Idle loop
                            loop {
                                // Wait for an interrupt to wake up
                                enable_external_interrupts();
                                idle();
                            }
                        });
                        kernel_task.init();
                        // Add idle task to the ready queue
                        self.ready_queue[cpu_id].push_back(kernel_task);
                    }
                }
            } else {
                match task {
                    Some(mut t) => {
                        match t.state {
                            TaskState::NotInitialized => {
                                panic!("Task must be initialized before scheduling");
                            },
                            TaskState::Zombie => {
                                let task_id = t.get_id();
                                let parent_id = t.get_parent_id();
                                self.zombie_queue[cpu_id].push_back(t);
                                // Wake up any processes waiting for this specific task
                                wake_task_waiters(task_id);
                                // Also wake up parent process for waitpid(-1)
                                if let Some(parent_id) = parent_id {
                                    wake_parent_waiters(parent_id);
                                }
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
                            TaskState::Ready | TaskState::Running => {

                                t.time_slice = 1; // Reset time slice on dispatch
                                let next_task_id = t.get_id();
                                self.current_task_id[cpu_id] = Some(next_task_id);
                                self.ready_queue[cpu_id].push_back(t);
                                return (old_current_task_id, Some(next_task_id));
                            }
                        }
                    }
                    None => return (old_current_task_id, self.current_task_id[cpu_id]),
                }
            }
        }
    }

    /// Called every timer tick. Decrements the current task's time_slice.
    /// If time_slice reaches 0, triggers a reschedule.
    pub fn on_tick(&mut self, cpu_id: usize) {
        if let Some(task_id) = self.get_current_task_id(cpu_id) {
            if let Some(task) = self.ready_queue[cpu_id].iter_mut().find(|t| t.get_id() == task_id) {
                if task.time_slice > 0 {
                    task.time_slice -= 1;
                }
                if task.time_slice == 0 {
                    // Time slice expired, trigger reschedule
                    let cpu = get_cpu();
                    self.schedule(cpu);
                }
            }
        } else {
            let cpu = get_cpu();
            self.schedule(cpu);
        }
    }

    /// Schedule tasks on the CPU with kernel context switching
    /// 
    /// This function performs cooperative scheduling by switching between task
    /// kernel contexts. It returns to the caller, allowing the trap handler
    /// to handle user space return.
    /// 
    /// # Arguments
    /// * `cpu` - The CPU architecture state
    pub fn schedule(&mut self, cpu: &mut Arch) {
        let cpu_id = cpu.get_cpuid();

        // Step 1: Run scheduling algorithm to get current and next task IDs
        let (current_task_id, next_task_id) = self.run(cpu);

        // Step 2: Check if a context switch is needed
        if next_task_id.is_some() && current_task_id != next_task_id {
            let next_task_id = next_task_id.expect("Next task ID should be valid");

            // Store current task's user state to VCPU
            if let Some(current_task_id) = current_task_id {
                let current_task = self.get_task_by_id(current_task_id).unwrap();
                current_task.vcpu.store(cpu);

                let next_task = self.get_task_by_id(next_task_id).unwrap();

                if next_task.kernel_context.get_entry_point() == 0 {
                    next_task.kernel_context.set_entry_point(Self::dispatch as u64);
                }

                // Perform kernel context switch
                self.kernel_context_switch(cpu_id, current_task_id, next_task_id);
                // NOTE: After this point, the current task will not execute until it is scheduled again

                // Restore trapframe of same task
                let current_task = self.get_task_by_id(current_task_id).unwrap();
                Self::setup_task_execution(cpu, current_task);
            } else {
                // No current task (e.g., first scheduling), just switch to next task
                let next_task = self.get_task_by_id(next_task_id).unwrap();
                next_task.state = TaskState::Running;
                Self::setup_task_execution(cpu, next_task);
            }
        }

        // Step 3: Setup task execution and process events (after context switch)
        if let Some(current_task) = self.get_current_task(cpu_id) {
            // Process pending events before dispatching task
            let _ = current_task.process_pending_events();
        }
        // Schedule returns - trap handler will call arch_switch_to_user_space()
    }

    fn dispatch() -> ! {
        let cpu = get_cpu();
        let current_task = get_scheduler().get_current_task(cpu.get_cpuid()).unwrap();
        Self::setup_task_execution(cpu, current_task);
        arch_switch_to_user_space(cpu.get_trapframe());
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

    pub fn get_current_task_id(&self, cpu_id: usize) -> Option<usize> {
        self.current_task_id[cpu_id]
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
        // crate::println!("Scheduler: Waking up task {}", task_id);
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

    /// Get IDs of all tasks across ready, blocked, and zombie queues
    ///
    /// This helper is used by subsystems (e.g., event broadcast) that need
    /// to target every task in the system without holding a mutable
    /// reference to the scheduler during delivery.
    pub fn get_all_task_ids(&self) -> alloc::vec::Vec<usize> {
        let mut ids = alloc::vec::Vec::new();
        // Ready tasks
        for q in &self.ready_queue {
            for t in q.iter() {
                ids.push(t.get_id());
            }
        }
        // Blocked tasks
        for q in &self.blocked_queue {
            for t in q.iter() {
                ids.push(t.get_id());
            }
        }
        // Zombie tasks
        for q in &self.zombie_queue {
            for t in q.iter() {
                ids.push(t.get_id());
            }
        }
        ids
    }

    /// Perform kernel context switch between tasks
    /// 
    /// This function handles the low-level kernel context switching between
    /// the current task and the next selected task.
    /// 
    /// # Arguments
    /// * `cpu_id` - The CPU ID
    /// * `from_task_id` - Current task ID
    /// * `to_task_id` - Next task ID
    fn kernel_context_switch(&mut self, cpu_id: usize, from_task_id: usize, to_task_id: usize) {
        if from_task_id != to_task_id {
            // Find tasks in ready queue
            let mut from_ctx_ptr: *mut crate::arch::KernelContext = core::ptr::null_mut();
            let mut to_ctx_ptr: *const crate::arch::KernelContext = core::ptr::null();
            
            // Find context pointers using iterators
            for task in self.ready_queue[cpu_id].iter_mut() {
                if task.get_id() == from_task_id {
                    from_ctx_ptr = &mut task.kernel_context as *mut crate::arch::KernelContext;
                } else if task.get_id() == to_task_id {
                    to_ctx_ptr = &task.kernel_context as *const crate::arch::KernelContext;
                }
            }
            
            if !from_ctx_ptr.is_null() && !to_ctx_ptr.is_null() {
                // Perform kernel context switch
                unsafe {
                    crate::arch::switch::switch_to(from_ctx_ptr, to_ctx_ptr);
                }
                // Execution resumes here when this task is rescheduled
            }
        }
    }

    /// Setup task execution by configuring hardware and user context
    /// 
    /// This replaces the old dispatcher functionality with a more direct approach.
    /// 
    /// # Arguments
    /// * `cpu` - The CPU architecture state
    /// * `task` - The task to setup for execution
    fn setup_task_execution(cpu: &mut Arch, task: &mut Task) {
        
        // Setup trap vector
        set_trapvector(get_trampoline_trap_vector());

        // Setup trapframe for hardware
        let trapframe = cpu.get_trapframe();
        trapframe.set_trap_handler(get_user_trap_handler());
        trapframe.set_next_address_space(task.vm_manager.get_asid());
        trapframe.set_kernel_stack(task.get_kernel_stack_bottom());

        task.vcpu.switch(cpu);

        set_next_mode(task.vcpu.get_mode());
        
        // Note: User context (VCPU) will be restored in schedule() after run() returns
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