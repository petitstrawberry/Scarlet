//! `ps` — report a snapshot of current tasks.
//!
//! Usage: `ps`  or  `ps -l` (long format)

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use crate::std::string::ToString;
use std::task::{self, TaskState};
use std::{format, println};

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: std::vec::Vec<std::string::String> = std::env::args().collect();
    let long = args.iter().any(|a| a == "-l" || a == "--long");

    let tasks = task::info();

    if tasks.is_empty() {
        println!("No tasks found.");
        return 0;
    }

    if long {
        // Long format: PID PPID TGID TYPE STATE CPU NAME EXIT
        let mut max_pid = 3;
        let mut max_ppid = 4;
        let mut max_tgid = 4;
        let mut max_name = 4;

        for t in &tasks {
            max_pid = max_pid.max(format!("{}", t.pid()).len());
            max_ppid = max_ppid.max(format!("{}", t.ppid()).len());
            max_tgid = max_tgid.max(format!("{}", t.tgid()).len());
            max_name = max_name.max(t.name().len());
        }

        println!(
            "{:>w_pid$} {:>w_ppid$} {:>w_tgid$} {:<4} {:<7} {:<3} {:<w_name$} {}",
            "PID",
            "PPID",
            "TGID",
            "TYPE",
            "STATE",
            "CPU",
            "NAME",
            "EXIT",
            w_pid = max_pid,
            w_ppid = max_ppid,
            w_tgid = max_tgid,
            w_name = max_name,
        );

        for t in &tasks {
            let exit_str = if t.state() == TaskState::Zombie {
                format!("{}", t.exit_status())
            } else {
                "-".to_string()
            };
            println!(
                "{:>w_pid$} {:>w_ppid$} {:>w_tgid$} {:<4} {:<7} {:<3} {:<w_name$} {}",
                t.pid(),
                t.ppid(),
                t.tgid(),
                t.task_type(),
                t.state(),
                t.cpu(),
                t.name(),
                exit_str,
                w_pid = max_pid,
                w_ppid = max_ppid,
                w_tgid = max_tgid,
                w_name = max_name,
            );
        }
    } else {
        // Default format: PID TYPE STATE CPU NAME
        let mut max_pid = 3;
        let mut max_name = 4;

        for t in &tasks {
            max_pid = max_pid.max(format!("{}", t.pid()).len());
            max_name = max_name.max(t.name().len());
        }

        println!(
            "{:>w_pid$} {:<4} {:<7} {:<3} {:<w_name$}",
            "PID",
            "TYPE",
            "STATE",
            "CPU",
            "NAME",
            w_pid = max_pid,
            w_name = max_name,
        );

        for t in &tasks {
            println!(
                "{:>w_pid$} {:<4} {:<7} {:<3} {:<w_name$}",
                t.pid(),
                t.task_type(),
                t.state(),
                t.cpu(),
                t.name(),
                w_pid = max_pid,
                w_name = max_name,
            );
        }
    }

    0
}
