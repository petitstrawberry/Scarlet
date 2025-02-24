//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

extern crate alloc;

use alloc::string::String;

pub struct Task {
    name: String,
    priority: u32,
}

impl Task {
    pub fn new(name: String, priority: u32) -> Self {
        Task { name, priority }
    }

    pub fn run(&self) {
    }

    pub fn set_priority(&mut self, priority: u32) {
        self.priority = priority;
    }

    pub fn get_priority(&self) -> u32 {
        self.priority
    }

    pub fn get_name(&self) -> &String {
        &self.name
    }
}