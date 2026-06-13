//! `top` — display dynamic real-time view of running tasks.
//!
//! In the current Scarlet terminal, escape sequences for screen clearing
//! are not available, so this samples once and prints one batch-style view.
//!
//! Usage:
//!   `top`                            — sorted by recent CPU usage (default)
//!   `top -p` / `top --sort pid`      — sort by PID
//!   `top -n` / `top --sort name`     — sort by command name
//!   `top -c` / `top --sort cpu`      — sort by CPU id
//!   `top -s` / `top --sort state`    — sort by state
//!   `top -m` / `top --sort mem`      — sort by PID descending (no mem info yet)
//!   `top --idle`                     — include per-CPU idle kernel tasks
//!
//! Column descriptions (Linux-esque):
//!   PID     — Process/Task ID
//!   USER    — K (kernel) / U (user)
//!   PR      — Priority placeholder (always '-')
//!   STAT    — Task state (R/S/D/Z/T)
//!   CPU     — CPU core the task is on
//!   %CPU    — Recent CPU use over the sampling interval
//!   TIME+   — Cumulative CPU time
//!   COMMAND — Task name / binary path

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use std::task::{self, TaskState, TaskType};
use std::thread;
use std::{format, println};

const SAMPLE_INTERVAL_MS: u64 = 1000;

#[derive(Clone, Copy)]
enum SortKey {
    Pid,
    PercentCpu,
    Cpu,
    Name,
    State,
    Type,
}

fn parse_sort_key(s: &str) -> SortKey {
    match s {
        "pid" => SortKey::Pid,
        "pcpu" | "%cpu" | "cpu%" => SortKey::PercentCpu,
        "cpu" | "c" => SortKey::Cpu,
        "name" | "n" => SortKey::Name,
        "state" | "s" => SortKey::State,
        "type" => SortKey::Type,
        _ => SortKey::PercentCpu,
    }
}

fn format_percent_per_mille(per_mille: u64) -> std::string::String {
    format!("{}.{:01}", per_mille / 10, per_mille % 10)
}

fn format_time_ns(ns: u64) -> std::string::String {
    let total_ms = ns / 1_000_000;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;

    if hours > 0 {
        format!("{}:{:02}:{:02}.{:03}", hours, mins, secs, ms)
    } else {
        format!("{:02}:{:02}.{:03}", mins, secs, ms)
    }
}

fn is_idle_task(task: &std::task::TaskInfo) -> bool {
    task.task_type() == TaskType::Kernel && task.name().starts_with("idle")
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

#[derive(Clone)]
struct TaskSample {
    task: std::task::TaskInfo,
    cpu_per_mille: u64,
}

fn previous_cpu_time(tasks: &[std::task::TaskInfo], pid: usize) -> Option<u64> {
    tasks
        .iter()
        .find(|task| task.pid() == pid)
        .map(|task| task.cpu_time_ns())
}

fn apply_sort(tasks: &mut [TaskSample], key: SortKey) {
    tasks.sort_by(|a, b| {
        let primary = match key {
            SortKey::Pid => a.task.pid().cmp(&b.task.pid()),
            SortKey::PercentCpu => b.cpu_per_mille.cmp(&a.cpu_per_mille),
            SortKey::Cpu => a.task.cpu().cmp(&b.task.cpu()),
            SortKey::Name => a.task.name().cmp(b.task.name()),
            SortKey::State => format!("{}", a.task.state()).cmp(&format!("{}", b.task.state())),
            SortKey::Type => {
                format!("{}", a.task.task_type()).cmp(&format!("{}", b.task.task_type()))
            }
        };
        primary.then_with(|| a.task.pid().cmp(&b.task.pid()))
    });
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: std::vec::Vec<std::string::String> = std::env::args().collect();

    // Parse sort key - default: recent %CPU.
    let mut sort_key = SortKey::PercentCpu;
    let show_idle = args.iter().any(|a| a == "--idle");
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

    let before_tasks = task::info();
    let before_cpu = task::cpu_usage();
    thread::sleep(Duration::from_millis(SAMPLE_INTERVAL_MS));
    let mut tasks = task::info();
    let after_cpu = task::cpu_usage();

    if !show_idle {
        tasks.retain(|task| !is_idle_task(task));
    }

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

    let (busy_per_mille, idle_per_mille, total_delta_ns, online_cpus) =
        match (before_cpu, after_cpu) {
            (Some(before), Some(after)) => {
                let busy_delta = after.busy_time_ns().saturating_sub(before.busy_time_ns());
                let idle_delta = after.idle_time_ns().saturating_sub(before.idle_time_ns());
                let total_delta = busy_delta.saturating_add(idle_delta);
                let busy = if total_delta == 0 {
                    0
                } else {
                    ((busy_delta as u128 * 1000) / total_delta as u128) as u64
                };
                let idle = if total_delta == 0 {
                    0
                } else {
                    ((idle_delta as u128 * 1000) / total_delta as u128) as u64
                };
                (busy, idle, total_delta, after.online_cpus())
            }
            _ => (0, 0, 0, 1),
        };

    // ── Summary header (Linux top style) ──
    println!(
        "Tasks: {:>3} total, {:>3} running, {:>3} sleeping, {:>3} stopped, {:>3} zombie",
        total, running, sleeping, stopped, zombies
    );
    println!("       {:>3} user,  {:>3} kernel", user_tasks, kernel_tasks);
    println!(
        "%Cpu(s): {:>5} busy, {:>5} idle",
        format_percent_per_mille(busy_per_mille),
        format_percent_per_mille(idle_per_mille),
    );
    println!();

    // ── Column layout ──
    let mut sorted: std::vec::Vec<TaskSample> = tasks
        .into_iter()
        .map(|task| {
            let previous =
                previous_cpu_time(&before_tasks, task.pid()).unwrap_or(task.cpu_time_ns());
            let delta = task.cpu_time_ns().saturating_sub(previous);
            let cpu_per_mille = if total_delta_ns == 0 {
                0
            } else {
                ((delta as u128 * online_cpus as u128 * 1000) / total_delta_ns as u128) as u64
            };
            TaskSample {
                task,
                cpu_per_mille,
            }
        })
        .collect();
    apply_sort(&mut sorted, sort_key);

    let mut max_pid = 3;
    let mut max_cmd = 7;
    let mut max_cpu = 4;
    let mut max_time = 5;

    for sample in &sorted {
        let t = &sample.task;
        max_pid = max_pid.max(format!("{}", t.pid()).len());
        max_cmd = max_cmd.max(t.name().len());
        max_cpu = max_cpu.max(format!("CPU{}", t.cpu()).len());
        max_time = max_time.max(format_time_ns(t.cpu_time_ns()).len());
    }

    println!(
        "{:>w_pid$} {:<4} {:<2} {:<5} {:<w_cpu$} {:>5} {:>w_time$} {:<w_cmd$} {}",
        "PID",
        "USER",
        "PR",
        "STAT",
        "CPU",
        "%CPU",
        "TIME+",
        "COMMAND",
        "TGID",
        w_pid = max_pid,
        w_cpu = max_cpu,
        w_time = max_time,
        w_cmd = max_cmd,
    );

    for sample in &sorted {
        let t = &sample.task;
        let user = match t.task_type() {
            TaskType::Kernel => "K",
            TaskType::User => "U",
        };
        let stat = format_stat(t.state());

        println!(
            "{:>w_pid$} {:<4} {:<2} {:<5} {:<w_cpu$} {:>5} {:>w_time$} {:<w_cmd$} {}",
            t.pid(),
            user,
            "-", // PR placeholder
            stat,
            format!("CPU{}", t.cpu()),
            format_percent_per_mille(sample.cpu_per_mille),
            format_time_ns(t.cpu_time_ns()),
            t.name(),
            t.tgid(),
            w_pid = max_pid,
            w_cpu = max_cpu,
            w_time = max_time,
            w_cmd = max_cmd,
        );
    }

    0
}
