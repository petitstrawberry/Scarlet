//! Dispatcher module.
//! 
//! The dispatcher module is responsible for dispatching tasks to the CPU.
//! Currently, the dispatcher is a simple dispatcher that runs the task.

use super::task::Task;

pub struct Dispatcher;

impl Dispatcher {
    pub const fn new() -> Self {
        Dispatcher
    }

    pub fn dispatch(&self, task: &Task) {
        task.run();
    }
}