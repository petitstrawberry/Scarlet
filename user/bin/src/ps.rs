//! `ps` — report a snapshot of current tasks.
//!
//! Usage:
//!   `ps`           — default format (PID, STAT, CPU, COMMAND)
//!   `ps -e`        — same as default (all processes)
//!   `ps -l` / `ps --long`  — long format (PID, PPID, TGID, STAT, CPU, COMMAND, EXIT)
//!   `ps -T`        — show threads (TGID column added)
//!
//! Sort options:
//!   `ps -p`                        — sort by PID (default)
//!   `ps --sort pid`                — sort by PID ascending
//!   `ps --sort -pid`               — sort by PID descending
//!   `ps --sort cpu`                — sort by CPU id
//!   `ps --sort name`               — sort by command name
//!   `ps --sort state`              — sort by state
//!   `ps --sort type`               — sort by task type

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::string::ToString;
use std::task::{self, TaskState, TaskType};
use std::{format, println};

/// How to sort the output.
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
        "cpu" => SortKey::Cpu,
        "name" => SortKey::Name,
        "state" => SortKey::State,
        "type" => SortKey::Type,
        _ => SortKey::Pid,
    }
}

/// Short STAT representation (default format).
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

/// Long STAT representation (includes kernel/user suffix like Linux).
fn format_stat_long(state: TaskState, task_type: TaskType) -> std::string::String {
    let base = match state {
        TaskState::Running => "R",
        TaskState::Ready => "R",
        TaskState::BlockedInterruptible => "S",
        TaskState::BlockedUninterruptible => "D",
        TaskState::Zombie => "Z",
        TaskState::Terminated => "T",
        TaskState::NotInitialized => "?",
    };
    let suffix = match task_type {
        TaskType::Kernel => "k",
        TaskType::User => "",
    };
    let mut s = std::string::String::new();
    s.push_str(base);
    s.push_str(suffix);
    s
}

/// Apply sorting to the task list.
fn apply_sort(tasks: &mut [std::task::TaskInfo], key: SortKey, ascending: bool) {
    let dir = |ord: core::cmp::Ordering| {
        if ascending { ord } else { ord.reverse() }
    };
    tasks.sort_by(|a, b| {
        let primary = match key {
            SortKey::Pid => a.pid().cmp(&b.pid()),
            SortKey::Cpu => a.cpu().cmp(&b.cpu()),
            SortKey::Name => a.name().cmp(b.name()),
            SortKey::State => format!("{}", a.state()).cmp(&format!("{}", b.state())),
            SortKey::Type => format!("{}", a.task_type()).cmp(&format!("{}", b.task_type())),
        };
        dir(primary).then_with(|| a.pid().cmp(&b.pid()))
    });
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: std::vec::Vec<std::string::String> = std::env::args().collect();

    let long = args.iter().any(|a| a == "-l" || a == "--long");
    let show_threads = args.iter().any(|a| a == "-T" || a == "--threads");

    // Parse sort key
    let mut sort_key = SortKey::Pid;
    let mut sort_ascending = true;
    for (i, a) in args.iter().enumerate() {
        if a == "-p" {
            sort_key = SortKey::Pid;
        } else if a == "--sort" {
            if let Some(key_str) = args.get(i + 1) {
                if key_str.starts_with('-') {
                    sort_ascending = false;
                    sort_key = parse_sort_key(&key_str[1..]);
                } else {
                    sort_key = parse_sort_key(key_str);
                }
            }
        }
    }

    let mut tasks = task::info();

    if tasks.is_empty() {
        println!("No tasks found.");
        return 0;
    }

    apply_sort(&mut tasks, sort_key, sort_ascending);

    if long {
        // ── Long format: PID PPID TGID STAT CPU COMMAND EXIT ──
        let mut max_pid = 3;
        let mut max_ppid = 4;
        let mut max_tgid = 4;
        let mut max_cmd = 7;
        let mut max_cpu = 4; // "CPU" header

        for t in &tasks {
            max_pid = max_pid.max(format!("{}", t.pid()).len());
            max_ppid = max_ppid.max(format!("{}", t.ppid()).len());
            max_tgid = max_tgid.max(format!("{}", t.tgid()).len());
            max_cmd = max_cmd.max(t.name().len());
            max_cpu = max_cpu.max(format!("CPU{}", t.cpu()).len());
        }

        println!(
            "{:>w_pid$} {:>w_ppid$} {:>w_tgid$} {:<5} {:<w_cpu$} {:<w_cmd$} {}",
            "PID",
            "PPID",
            "TGID",
            "STAT",
            "CPU",
            "COMMAND",
            "EXIT",
            w_pid = max_pid,
            w_ppid = max_ppid,
            w_tgid = max_tgid,
            w_cpu = max_cpu,
            w_cmd = max_cmd,
        );

        for t in &tasks {
            let exit_str = if t.state() == TaskState::Zombie {
                format!("{}", t.exit_status())
            } else {
                "-".to_string()
            };
            let stat = format_stat_long(t.state(), t.task_type());
            println!(
                "{:>w_pid$} {:>w_ppid$} {:>w_tgid$} {:<5} {:<w_cpu$} {:<w_cmd$} {}",
                t.pid(),
                t.ppid(),
                t.tgid(),
                stat,
                format!("CPU{}", t.cpu()),
                t.name(),
                exit_str,
                w_pid = max_pid,
                w_ppid = max_ppid,
                w_tgid = max_tgid,
                w_cpu = max_cpu,
                w_cmd = max_cmd,
            );
        }
    } else if show_threads {
        // ── Thread format: PID TGID STAT CPU COMMAND ──
        let mut max_pid = 3;
        let mut max_tgid = 4;
        let mut max_cmd = 7;
        let mut max_cpu = 4;

        for t in &tasks {
            max_pid = max_pid.max(format!("{}", t.pid()).len());
            max_tgid = max_tgid.max(format!("{}", t.tgid()).len());
            max_cmd = max_cmd.max(t.name().len());
            max_cpu = max_cpu.max(format!("CPU{}", t.cpu()).len());
        }

        println!(
            "{:>w_pid$} {:>w_tgid$} {:<5} {:<w_cpu$} {:<w_cmd$}",
            "PID",
            "TGID",
            "STAT",
            "CPU",
            "COMMAND",
            w_pid = max_pid,
            w_tgid = max_tgid,
            w_cpu = max_cpu,
            w_cmd = max_cmd,
        );

        for t in &tasks {
            let stat = format_stat(t.state());
            println!(
                "{:>w_pid$} {:>w_tgid$} {:<5} {:<w_cpu$} {:<w_cmd$}",
                t.pid(),
                t.tgid(),
                stat,
                format!("CPU{}", t.cpu()),
                t.name(),
                w_pid = max_pid,
                w_tgid = max_tgid,
                w_cpu = max_cpu,
                w_cmd = max_cmd,
            );
        }
    } else {
        // ── Default format: PID STAT CPU COMMAND ──
        let mut max_pid = 3;
        let mut max_cmd = 7;
        let mut max_cpu = 4;

        for t in &tasks {
            max_pid = max_pid.max(format!("{}", t.pid()).len());
            max_cmd = max_cmd.max(t.name().len());
            max_cpu = max_cpu.max(format!("CPU{}", t.cpu()).len());
        }

        println!(
            "{:>w_pid$} {:<5} {:<w_cpu$} {:<w_cmd$}",
            "PID",
            "STAT",
            "CPU",
            "COMMAND",
            w_pid = max_pid,
            w_cpu = max_cpu,
            w_cmd = max_cmd,
        );

        for t in &tasks {
            let stat = format_stat(t.state());
            println!(
                "{:>w_pid$} {:<5} {:<w_cpu$} {:<w_cmd$}",
                t.pid(),
                stat,
                format!("CPU{}", t.cpu()),
                t.name(),
                w_pid = max_pid,
                w_cpu = max_cpu,
                w_cmd = max_cmd,
            );
        }
    }

    0
}
