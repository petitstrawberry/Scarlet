use std::env;
use std::process::ExitCode;

use scarlet_sys::{Syscall, syscall2};

fn signal_number(value: &str) -> Option<usize> {
    let normalized = value
        .strip_prefix("SIG")
        .or_else(|| value.strip_prefix("sig"))
        .unwrap_or(value);
    match normalized.to_ascii_uppercase().as_str() {
        "HUP" => Some(1),
        "INT" => Some(2),
        "QUIT" => Some(3),
        "KILL" => Some(9),
        "TERM" => Some(15),
        "CONT" => Some(18),
        "STOP" => Some(19),
        "TSTP" => Some(20),
        "TTIN" => Some(21),
        "TTOU" => Some(22),
        "WINCH" => Some(28),
        _ => normalized.parse().ok(),
    }
}

fn usage() {
    println!("Usage: kill [-SIGNAL | -s SIGNAL] PID...");
    println!("       kill -9 PID    force termination");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
        return ExitCode::from(2);
    }

    let mut signal = 15usize;
    let mut first_pid = 0usize;
    if args[0] == "-s" {
        let Some(value) = args.get(1) else {
            usage();
            return ExitCode::from(2);
        };
        let Some(parsed) = signal_number(value) else {
            println!("kill: invalid signal: {value}");
            return ExitCode::from(2);
        };
        signal = parsed;
        first_pid = 2;
    } else if let Some(value) = args[0].strip_prefix('-') {
        let Some(parsed) = signal_number(value) else {
            println!("kill: invalid signal: {value}");
            return ExitCode::from(2);
        };
        signal = parsed;
        first_pid = 1;
    }

    if first_pid == args.len() {
        usage();
        return ExitCode::from(2);
    }

    let mut failed = false;
    for value in &args[first_pid..] {
        let Ok(pid) = value.parse::<usize>() else {
            println!("kill: invalid PID or TID: {value}");
            failed = true;
            continue;
        };
        if syscall2(Syscall::Kill, pid, signal) == usize::MAX {
            println!("kill: failed to signal {pid}");
            failed = true;
        }
    }

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
