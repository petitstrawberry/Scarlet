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

use core::panic;

use alloc::{boxed::Box, collections::vec_deque::VecDeque, string::ToString, vec::Vec};
use hashbrown::HashMap;

use crate::{arch::{Arch, Trapframe, enable_interrupt, get_cpu, get_user_trap_handler, instruction::idle, interrupt::enable_external_interrupts, set_arch, set_next_mode, set_trapvector, trap::{self, user::arch_switch_to_user_space}}, environment::NUM_OF_CPUS, task::{TaskState, new_kernel_task, wake_parent_waiters, wake_task_waiters}, timer::get_kernel_timer, vm::{get_kernel_vm_manager, get_trampoline_arch, get_trampoline_trap_vector}};
use crate::println;
use crate::print;

use crate::task::Task;

/// Task pool that stores tasks in fixed positions
const MAX_TASKS: usize = 1024;

struct TaskPool {
    // Fixed-length slice on heap
    tasks: Box<[Option<Task>]>,
    id_to_index: HashMap<usize, usize>,
    free_indices: Vec<usize>,
    next_free_index: usize,
}

impl TaskPool {
    fn new() -> Self {
        // Create fixed-length slice on heap
        let tasks: Box<[Option<Task>]> = (0..MAX_TASKS)
            .map(|_| None)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        
        TaskPool {
            tasks,
            id_to_index: HashMap::new(),
            free_indices: Vec::new(),
            next_free_index: 0,
        }
    }

    fn add_task(&mut self, task: Task) -> Result<(), &'static str> {
        let task_id = task.get_id();
        
        // Find available index
        let index = if let Some(free_idx) = self.free_indices.pop() {
            free_idx
        } else if self.next_free_index < self.tasks.len() {
            let idx = self.next_free_index;
            self.next_free_index += 1;
            idx
        } else {
            return Err("Task pool full");
        };
        
        self.tasks[index] = Some(task);
        self.id_to_index.insert(task_id, index);
        Ok(())
    }

    fn get_task(&mut self, task_id: usize) -> Option<&mut Task> {
        let index = *self.id_to_index.get(&task_id)?;
        self.tasks.get_mut(index)?.as_mut()
    }

    fn remove_task(&mut self, task_id: usize) -> Option<Task> {
        let index = self.id_to_index.remove(&task_id)?;
        let task = self.tasks[index].take()?;
        self.free_indices.push(index);
        Some(task)
    }

    #[allow(dead_code)]
    fn contains_task(&self, task_id: usize) -> bool {
        self.id_to_index.contains_key(&task_id)
    }
}

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
    /// Task pool storing all tasks in fixed positions
    task_pool: TaskPool,
    /// Queue for ready-to-run task IDs
    ready_queue: [VecDeque<usize>; NUM_OF_CPUS],
    /// Queue for blocked task IDs (waiting for I/O, etc.)
    blocked_queue: [VecDeque<usize>; NUM_OF_CPUS],
    /// Queue for zombie task IDs (finished but not yet cleaned up)
    zombie_queue: [VecDeque<usize>; NUM_OF_CPUS],
    current_task_id: [Option<usize>; NUM_OF_CPUS],
}

impl Scheduler {
    pub fn new() -> Self {
        Scheduler {
            task_pool: TaskPool::new(),
            ready_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            blocked_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            zombie_queue: [const { VecDeque::new() }; NUM_OF_CPUS],
            current_task_id: [const { None }; NUM_OF_CPUS],
        }
    }

    pub fn add_task(&mut self, task: Task, cpu_id: usize) {
        let task_id = task.get_id();
        // Add task to the task pool
        if let Err(e) = self.task_pool.add_task(task) {
            panic!("Failed to add task {}: {}", task_id, e);
        }
        // Add task state info to ready queue
        self.ready_queue[cpu_id].push_back(task_id);
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
    fn run(&mut self, cpu: &Arch) -> (Option<usize>, Option<usize>) {
        let cpu_id = cpu.get_cpuid();
        let old_current_task_id = self.current_task_id[cpu_id];

        // Continue trying to find a suitable task to run
        loop {
            let task_id = self.ready_queue[cpu_id].pop_front();

            /* If there are no subsequent tasks */
            if self.ready_queue[cpu_id].is_empty() {
                match task_id {
                    Some(task_id) => {
                        let t = self.get_task_by_id(task_id).expect("Task must exist in task pool");
                        match t.state {
                            TaskState::NotInitialized => {
                                panic!("Task must be initialized before scheduling");
                            },
                            TaskState::Zombie => {
                                let task_id = t.get_id();
                                let parent_id = t.get_parent_id();
                                self.zombie_queue[cpu_id].push_back(task_id);
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
                                if self.current_task_id[cpu_id] == Some(task_id) {
                                    self.current_task_id[cpu_id] = None;
                                }
                                // Put blocked task to blocked queue without running it
                                self.blocked_queue[cpu_id].push_back(task_id);
                                continue;
                            },
                            TaskState::Ready | TaskState::Running => {
                                t.state = TaskState::Running;
                                // Task is ready to run
                                t.time_slice = 1; // Reset time slice on dispatch
                                let next_task_id = t.get_id();
                                self.current_task_id[cpu_id] = Some(next_task_id);
                                self.ready_queue[cpu_id].push_back(task_id);
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
                        self.add_task(kernel_task, cpu_id);
                    }
                }
            } else {
                match task_id {
                    Some(task_id) => {
                        let t = self.get_task_by_id(task_id).expect("Task must exist in task pool");
                        match t.state {
                            TaskState::NotInitialized => {
                                panic!("Task must be initialized before scheduling");
                            },
                            TaskState::Zombie => {
                                let task_id = t.get_id();
                                let parent_id = t.get_parent_id();
                                self.zombie_queue[cpu_id].push_back(task_id);
                                // Wake up any processes waiting for this specific task
                                wake_task_waiters(task_id);
                                // Also wake up parent process for waitpid(-1)
                                if let Some(parent_id) = parent_id {
                                    wake_parent_waiters(parent_id);
                                }
                                continue;
                            },
                            TaskState::Terminated => {
                                self.task_pool.remove_task(task_id);
                                continue;
                            },
                            TaskState::Blocked(_) => {
                                // Reset current_task_id since this task is no longer current
                                if self.current_task_id[cpu_id] == Some(task_id) {
                                    self.current_task_id[cpu_id] = None;
                                }
                                // Put blocked task back to the end of queue without running it
                                self.blocked_queue[cpu_id].push_back(task_id);
                                continue;
                            },
                            TaskState::Ready | TaskState::Running => {

                                t.time_slice = 1; // Reset time slice on dispatch
                                let next_task_id = t.get_id();
                                self.current_task_id[cpu_id] = Some(next_task_id);
                                self.ready_queue[cpu_id].push_back(task_id);
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
    pub fn on_tick(&mut self, cpu_id: usize, trapframe: &mut Trapframe) {
        if let Some(task_id) = self.get_current_task_id(cpu_id) {
            if let Some(task) = self.task_pool.get_task(task_id) {
                if task.time_slice > 0 {
                    task.time_slice -= 1;
                }
                if task.time_slice == 0 {
                    // Time slice expired, trigger reschedule
                    self.schedule(trapframe);
                }
            }
        } else {
            self.schedule(trapframe);
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
    pub fn schedule(&mut self, trapframe: &mut Trapframe) {
        let cpu = get_cpu();
        let cpu_id = cpu.get_cpuid();

        // Step 1: Run scheduling algorithm to get current and next task IDs
        let (current_task_id, next_task_id) = self.run(cpu);

        // Debug output for monitoring scheduler behavior
        // if let Some(current_id) = current_task_id {
        //     if let Some(next_id) = next_task_id {
        //         if current_id != next_id {
        //             crate::println!("[SCHED] CPU{}: Task {} -> Task {}", cpu_id, current_id, next_id);
        //         }
        //     } else {
        //         crate::println!("[SCHED] CPU{}: Task {} -> idle", cpu_id, current_id);
        //     }
        // } else if let Some(next_id) = next_task_id {
        //     crate::println!("[SCHED] CPU{}: idle -> Task {}", cpu_id, next_id);
        // }

        // Step 2: Check if a context switch is needed
        if next_task_id.is_some() && current_task_id != next_task_id {
            let next_task_id = next_task_id.expect("Next task ID should be valid");

            // Store current task's user state to VCPU
            if let Some(current_task_id) = current_task_id {
                let current_task = self.get_task_by_id(current_task_id).unwrap();
                current_task.vcpu.store(trapframe);

                // Perform kernel context switch
                self.kernel_context_switch(cpu_id, current_task_id, next_task_id);
                // NOTE: After this point, the current task will not execute until it is scheduled again

                // Restore trapframe of same task
                let current_task = self.get_task_by_id(current_task_id).unwrap();
                Self::setup_task_execution(get_cpu(), current_task);
            } else {            // No current task (e.g., first scheduling), just switch to next task
                let next_task = self.get_task_by_id(next_task_id).unwrap();
                // crate::println!("[SCHED] Setting up task {} for execution", next_task_id);
                Self::setup_task_execution(get_cpu(), next_task);
                arch_switch_to_user_space(get_cpu().get_trapframe()); // Force switch to user space
            }
        }

        // Step 3: Setup task execution and process events (after context switch)
        if let Some(current_task) = self.get_current_task(cpu_id) {
            // Process pending events before dispatching task
            let _ = current_task.process_pending_events();
        }
        // Schedule returns - trap handler will call arch_switch_to_user_space()
    }


    /* MUST NOT raise any exception in this function before the idle loop */
    pub fn start_scheduler(&mut self) {
        let cpu = get_cpu();
        let cpu_id = cpu.get_cpuid();
        let timer = get_kernel_timer();
        timer.stop(cpu_id);

        let trap_vector = get_trampoline_trap_vector();
        let arch = get_trampoline_arch(cpu_id);
        set_trapvector(trap_vector);
        set_arch(arch);
        cpu.set_trap_handler(get_user_trap_handler());
        cpu.set_next_address_space(get_kernel_vm_manager().get_asid());

        /* Jump to trap handler immediately */
        timer.set_interval_us(cpu_id, 0);
        enable_interrupt();
        timer.start(cpu_id);
        idle();
    }

    pub fn get_current_task(&mut self, cpu_id: usize) -> Option<&mut Task> {
        match self.current_task_id[cpu_id] {
            Some(task_id) => self.task_pool.get_task(task_id),
            None => None
        }
    }

    pub fn get_current_task_id(&self, cpu_id: usize) -> Option<usize> {
        self.current_task_id[cpu_id]
    }

    /// Returns a mutable reference to the task with the specified ID, if found.
    /// 
    /// This method searches the TaskPool to find the task with the specified ID.
    /// This is needed for Waker integration.
    /// 
    /// # Arguments
    /// * `task_id` - The ID of the task to search for.
    /// 
    /// # Returns
    /// A mutable reference to the task if found, or None otherwise.
    pub fn get_task_by_id(&mut self, task_id: usize) -> Option<&mut Task> {
        self.task_pool.get_task(task_id)
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
        // Search for the task in blocked queues
        for cpu_id in 0..self.blocked_queue.len() {
            if let Some(pos) = self.blocked_queue[cpu_id].iter().position(|&id| id == task_id) {
                // Remove from blocked queue
                self.blocked_queue[cpu_id].remove(pos);
                
                // Get task from TaskPool and set state to Running
                if let Some(task) = self.task_pool.get_task(task_id) {
                    task.state = TaskState::Running;
                    // Move to ready queue
                    self.ready_queue[cpu_id].push_back(task_id);
                    return true;
                }
            }
        }
        // Not found in blocked queues. This can happen if a wake occurs between
        // a task marking itself Blocked and the scheduler moving it to the
        // blocked_queue. In that case, ensure the task state is set back to
        // Running so that the scheduler does not park it.
        if let Some(task) = self.task_pool.get_task(task_id) {
            if let TaskState::Blocked(_) = task.state {
                task.state = TaskState::Running;
                // Do not enqueue here to avoid duplicating entries: the task is
                // still present in the ready_queue (or is current) and will be
                // handled as Running by the scheduler.
                return true;
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
                ids.push(*t);
            }
        }
        // Blocked tasks
        for q in &self.blocked_queue {
            for t in q.iter() {
                ids.push(*t);
            }
        }
        // Zombie tasks
        for q in &self.zombie_queue {
            for t in q.iter() {
                ids.push(*t);
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
    fn kernel_context_switch(&mut self, _cpu_id: usize, from_task_id: usize, to_task_id: usize) {
        // crate::println!("[SCHED] CPU{}: Switching kernel context from Task {} to Task {}", cpu_id, from_task_id, to_task_id);
        if from_task_id != to_task_id {
            // Find tasks in all queues (ready, blocked, zombie)
            let mut from_ctx_ptr: *mut crate::arch::KernelContext = core::ptr::null_mut();
            let mut to_ctx_ptr: *const crate::arch::KernelContext = core::ptr::null();
            
            if let Some(from_task) = self.task_pool.get_task(from_task_id) {
                from_ctx_ptr = &mut from_task.kernel_context
            }
            if let Some(to_task) = self.task_pool.get_task(to_task_id) {
                to_ctx_ptr = &to_task.kernel_context
            }
            
            if !from_ctx_ptr.is_null() && !to_ctx_ptr.is_null() {
                // Perform kernel context switch
                unsafe {
                    crate::arch::switch::switch_to(from_ctx_ptr, to_ctx_ptr);
                }
                // Execution resumes here when this task is rescheduled
            } else {
                // crate::println!("[SCHED] ERROR: Context pointers not found - from: {:p}, to: {:p}", from_ctx_ptr, to_ctx_ptr);
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
    pub fn setup_task_execution(cpu: &mut Arch, task: &mut Task) {

        // crate::early_println!("[SCHED] Setting up Task {} for execution", task.get_id());
        // crate::early_println!("[SCHED]   before CPU {:#x?}", cpu);
        // let trapframe = cpu.get_trapframe();
        // crate::early_println!("[SCHED]   before Trapframe {:#x?}", trapframe);

        cpu.set_kernel_stack(task.get_kernel_stack_bottom());
        let trappframe = cpu.get_trapframe();

        task.vcpu.switch(trappframe);

        cpu.set_trap_handler(get_user_trap_handler());
        cpu.set_next_address_space(task.vm_manager.get_asid());
        set_next_mode(task.vcpu.get_mode());
        // Setup trap vector
        set_trapvector(get_trampoline_trap_vector());

        // crate::early_println!("[SCHED]   after  CPU {:#x?}", cpu);
        // crate::early_println!("[SCHED]   after  Trapframe {:#x?}", cpu.get_trapframe());

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