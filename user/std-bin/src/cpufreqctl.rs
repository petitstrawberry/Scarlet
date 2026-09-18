//! Common CPU frequency policy control utility.

use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    process::ExitCode,
};

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    let (command, cpu) = match args.as_slice() {
        [] => (None, "0"),
        [command] if command == "get" => (None, "0"),
        [operation, value] if operation == "set" || operation == "governor" => {
            (Some((operation.as_str(), value.as_str())), "0")
        }
        [operation, value, cpu] if operation == "set" || operation == "governor" => {
            (Some((operation.as_str(), value.as_str())), cpu.as_str())
        }
        _ => return Err("usage: cpufreqctl [get|set <kHz> [cpu]|governor <name> [cpu]]".into()),
    };
    if let Some((operation, value)) = command {
        cpu.parse::<usize>().map_err(|_| "invalid CPU ID")?;
        if operation == "set" {
            value
                .parse::<u64>()
                .map_err(|_| "frequency must be an integer in kHz")?;
        } else if !matches!(
            value,
            "performance" | "powersave" | "userspace" | "schedutil"
        ) {
            return Err("unknown governor".into());
        }
        let operation = if operation == "set" {
            "frequency"
        } else {
            operation
        };
        let mut device = OpenOptions::new()
            .write(true)
            .open("/dev/cpufreq")
            .map_err(|error| error.to_string())?;
        device
            .write_all(format!("cpu {cpu} {operation} {value}\n").as_bytes())
            .map_err(|error| error.to_string())?;
    }
    print!(
        "{}",
        fs::read_to_string("/dev/cpufreq").map_err(|error| error.to_string())?
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cpufreqctl: {error}");
            ExitCode::from(1)
        }
    }
}
