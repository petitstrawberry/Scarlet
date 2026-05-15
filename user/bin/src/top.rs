//! `top` — display dynamic real-time view of running tasks.
//!
//! In the current Scarlet terminal, escape sequences for screen clearing
//! are not available, so this shows a single snapshot (like `top -b -n 1`).
//!
//! Usage:
//!   `top`                            — sorted by CPU core (default)
//!   `top -p` / `top --sort pid`      — sort by PID
//!   `top -n` / `top --sort name`     — sort by command name
//!   `top -c` / `top --sort cpu`      — sort by CPU id
//!   `top -s` / `top --sort state`    — sort by state
//!   `top -m` / `top --sort mem`      — sort by PID descending (no mem info yet)
//!
//! Column descriptions (Linux-esque):
//!   PID     — Process/Task ID
//!   USER    — K (kernel) / U (user)
//!   PR      — Priority placeholder (always '-')
//!   STAT    — Task state (R/S/D/Z/T)
//!   CPU     — CPU core the task is on
//!   TIME+   — Placeholder (not tracked yet)
//!   COMMAND — Task name / binary path

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::string::ToString;
use std::task::{self, TaskState, TaskType};
use std::{format, println};

#[derive(Clone, Copy)]
enum SortKey {
    Pid,
    Cpu,
    Name,
    State,
    Type,
}

fn parse_sort_key(s: &str) -> SortKey {
    match s {
        "pid" => SortKey::Pid,
        "cpu" | "c" => SortKey::Cpu,
        "name" | "n" => SortKey::Name,
        "state" | "s" => SortKey::State,
        "type" => SortKey::Type,
        _ => SortKey::Cpu,
    }
}

fn format_stat(state: TaskState) -> &'static str {
    match state {
        TaskState::Running => "R",
        TaskState::Ready => "R<",
        TaskState::BlockedInterruptible => "S",
        TaskState::BlockedUninterruptible => "D",
        TaskState::Zombie => "Z",
        TaskState::Terminated => "T",
        TaskState::NotInitialized => "?",
    }
}

fn apply_sort(tasks: &mut [std::task::TaskInfo], key: SortKey) {
    tasks.sort_by(|a, b| {
        let primary = match key {
            SortKey::Pid => a.pid().cmp(&b.pid()),
            SortKey::Cpu => a.cpu().cmp(&b.cpu()),
            SortKey::Name => a.name().cmp(b.name()),
            SortKey::State => format!("{}", a.state()).cmp(&format!("{}", b.state())),
            SortKey::Type => format!("{}", a.task_type()).cmp(&format!("{}", b.task_type())),
        };
        primary.then_with(|| a.pid().cmp(&b.pid()))
    });
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: std::vec::Vec<std::string::String> = std::env::args().collect();

    // Parse sort key — default: CPU
    let mut sort_key = SortKey::Cpu;
    for (i, a) in args.iter().enumerate() {
        match a.as_str() {
            "-p" => sort_key = SortKey::Pid,
            "-n" => sort_key = SortKey::Name,
            "-c" => sort_key = SortKey::Cpu,
            "-s" => sort_key = SortKey::State,
            "-m" => sort_key = SortKey::Pid, // placeholder
            "--sort" => {
                if let Some(v) = args.get(i + 1) {
                    sort_key = parse_sort_key(v);
                }
            }
            _ => {}
        }
    }

    let tasks = task::info();

    if tasks.is_empty() {
        println!("No tasks found.");
        return 0;
    }

    let total = tasks.len();
    let running = tasks
        .iter()
        .filter(|t| t.state() == TaskState::Running)
        .count();
    let sleeping = tasks
        .iter()
        .filter(|t| t.state() == TaskState::BlockedInterruptible)
        .count();
    let stopped = tasks
        .iter()
        .filter(|t| t.state() == TaskState::Terminated)
        .count();
    let zombies = tasks
        .iter()
        .filter(|t| t.state() == TaskState::Zombie)
        .count();
    let user_tasks = tasks
        .iter()
        .filter(|t| t.task_type() == TaskType::User)
        .count();
    let kernel_tasks = tasks
        .iter()
        .filter(|t| t.task_type() == TaskType::Kernel)
        .count();

    // ── Summary header (Linux top style) ──
    println!(
        "Tasks: {:>3} total, {:>3} running, {:>3} sleeping, {:>3} stopped, {:>3} zombie",
        total, running, sleeping, stopped, zombies
    );
    println!("       {:>3} user,  {:>3} kernel", user_tasks, kernel_tasks);
    println!();

    // ── Column layout ──
    let mut sorted = tasks;
    apply_sort(&mut sorted, sort_key);

    let mut max_pid = 3;
    let mut max_cmd = 7;
    let mut max_cpu = 4;

    for t in &sorted {
        max_pid = max_pid.max(format!("{}", t.pid()).len());
        max_cmd = max_cmd.max(t.name().len());
        max_cpu = max_cpu.max(format!("CPU{}", t.cpu()).len());
    }

    println!(
        "{:>w_pid$} {:<4} {:<2} {:<5} {:<w_cpu$} {:<5} {:<w_cmd$} {}",
        "PID",
        "USER",
        "PR",
        "STAT",
        "CPU",
        "TIME+",
        "COMMAND",
        "TGID",
        w_pid = max_pid,
        w_cpu = max_cpu,
        w_cmd = max_cmd,
    );

    for t in &sorted {
        let user = match t.task_type() {
            TaskType::Kernel => "K",
            TaskType::User => "U",
        };
        let stat = format_stat(t.state());
        let exit_str = if t.state() == TaskState::Zombie {
            format!("{}", t.exit_status())
        } else {
            "  -".to_string()
        };

        println!(
            "{:>w_pid$} {:<4} {:<2} {:<5} {:<w_cpu$} {:<5} {:<w_cmd$} {}",
            t.pid(),
            user,
            "-", // PR placeholder
            stat,
            format!("CPU{}", t.cpu()),
            exit_str, // TIME+ placeholder — reuse for TGID when zombie
            t.name(),
            t.tgid(),
            w_pid = max_pid,
            w_cpu = max_cpu,
            w_cmd = max_cmd,
        );
    }

    0
}
