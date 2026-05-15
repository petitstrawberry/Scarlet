//! `top` — display dynamic real-time view of running tasks.
//!
//! In the current Scarlet terminal, escape sequences for screen clearing
//! are not available, so this shows a single snapshot (like a one-shot `top -b -n 1`).
//!
//! Usage: `top`

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use crate::std::string::ToString;
use std::task::{self, TaskType};
use std::{format, println};

#[unsafe(no_mangle)]
fn main() -> i32 {
    let tasks = task::info();

    if tasks.is_empty() {
        println!("No tasks found.");
        return 0;
    }

    let total = tasks.len();
    let running = tasks
        .iter()
        .filter(|t| t.state() == std::task::TaskState::Running)
        .count();
    let sleeping = tasks
        .iter()
        .filter(|t| t.state() == std::task::TaskState::BlockedInterruptible)
        .count();
    let zombies = tasks
        .iter()
        .filter(|t| t.state() == std::task::TaskState::Zombie)
        .count();
    let user_tasks = tasks
        .iter()
        .filter(|t| t.task_type() == TaskType::User)
        .count();
    let kernel_tasks = tasks
        .iter()
        .filter(|t| t.task_type() == TaskType::Kernel)
        .count();

    // Summary
    println!(
        "Tasks: {} total, {} running, {} sleeping, {} zombie",
        total, running, sleeping, zombies
    );
    println!("       {} user, {} kernel", user_tasks, kernel_tasks);
    println!();

    // Column layout — sorted by PID
    let mut sorted = tasks;
    sorted.sort_by_key(|t| t.pid());

    let mut max_pid = 3;
    let mut max_name = 4;

    for t in &sorted {
        max_pid = max_pid.max(format!("{}", t.pid()).len());
        max_name = max_name.max(t.name().len());
    }

    println!(
        "{:>w_pid$} {:<4} {:<8} {:<3} {:>5} {:<w_name$} {}",
        "PID",
        "TYPE",
        "STATE",
        "CPU",
        "TGID",
        "COMMAND",
        "EXIT",
        w_pid = max_pid,
        w_name = max_name,
    );

    for t in &sorted {
        let exit_str = if t.state() == std::task::TaskState::Zombie {
            format!("{}", t.exit_status())
        } else {
            "  -".to_string()
        };
        println!(
            "{:>w_pid$} {:<4} {:<8} {:<3} {:>5} {:<w_name$} {}",
            t.pid(),
            t.task_type(),
            t.state(),
            t.cpu(),
            t.tgid(),
            t.name(),
            exit_str,
            w_pid = max_pid,
            w_name = max_name,
        );
    }

    0
}
