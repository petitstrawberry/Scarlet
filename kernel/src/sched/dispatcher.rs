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
}