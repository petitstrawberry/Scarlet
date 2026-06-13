use std::env;
use std::io::{self, Write};
use std::process::{Command, ExitCode, Output};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);
const MIN_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_ROWS: usize = 24;
const DEFAULT_COLUMNS: usize = 80;

fn main() -> ExitCode {
    let options = match parse_args(env::args().skip(1)) {
        Ok(ParseResult::Run(options)) => options,
        Ok(ParseResult::Help) => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            println!("watch: {err}");
            print_usage();
            return ExitCode::from(1);
        }
    };

    loop {
        let started_at = Instant::now();
        let rows = terminal_size("LINES", DEFAULT_ROWS);
        let columns = terminal_size("COLUMNS", DEFAULT_COLUMNS);
        let mut rows_left = rows;
        let output = Command::new(&options.command).args(&options.args).output();

        if options.clear
            && let Err(err) = clear_screen()
        {
            println!("watch: failed to clear screen: {err}");
        }

        if options.show_title {
            println!(
                "Every {}s: {}",
                format_interval(options.interval),
                options.command_line()
            );
            println!();
            rows_left = rows_left.saturating_sub(2);
        }

        if let Err(err) = io::stdout().flush() {
            println!("watch: failed to flush stdout: {err}");
        }

        match output {
            Ok(output) => print_output(&output, rows_left, columns),
            Err(err) => {
                println!("watch: failed to execute '{}': {err}", options.command);
            }
        }

        let elapsed = started_at.elapsed();
        if elapsed < options.interval {
            thread::sleep(options.interval - elapsed);
        }
    }
}

struct Options {
    interval: Duration,
    show_title: bool,
    clear: bool,
    command: String,
    args: Vec<String>,
}

impl Options {
    fn command_line(&self) -> String {
        let mut command_line = self.command.clone();
        for arg in &self.args {
            command_line.push(' ');
            command_line.push_str(arg);
        }
        command_line
    }
}

enum ParseResult {
    Run(Options),
    Help,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<ParseResult, String> {
    let mut interval = DEFAULT_INTERVAL;
    let mut show_title = true;
    let mut clear = true;
    let mut command_parts = Vec::new();
    let mut parsing_options = true;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        if parsing_options {
            match arg.as_str() {
                "-h" | "--help" => return Ok(ParseResult::Help),
                "-n" | "--interval" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "option requires an argument: -n".to_string())?;
                    interval = parse_interval(&value)?;
                    continue;
                }
                "-t" | "--no-title" => {
                    show_title = false;
                    continue;
                }
                "--no-clear" => {
                    clear = false;
                    continue;
                }
                "--" => {
                    parsing_options = false;
                    continue;
                }
                _ => {}
            }
        }

        command_parts.push(arg);
        command_parts.extend(args);
        break;
    }

    if command_parts.is_empty() {
        return Err("missing command".to_string());
    }

    let command = command_parts.remove(0);

    Ok(ParseResult::Run(Options {
        interval,
        show_title,
        clear,
        command,
        args: command_parts,
    }))
}

fn parse_interval(value: &str) -> Result<Duration, String> {
    if value.is_empty() || value.starts_with('-') {
        return Err(format!("invalid interval '{value}'"));
    }

    let (seconds, nanos) = match value.split_once('.') {
        Some((seconds, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(format!("invalid interval '{value}'"));
            }

            let mut nanos = 0_u32;
            let mut scale = 100_000_000_u32;
            for byte in fraction.bytes().take(9) {
                nanos += u32::from(byte - b'0') * scale;
                scale /= 10;
            }

            (parse_seconds(seconds, value)?, nanos)
        }
        None => (parse_seconds(value, value)?, 0),
    };

    let interval = Duration::new(seconds, nanos);
    if interval < MIN_INTERVAL {
        return Err(format!(
            "interval must be at least {}s",
            format_interval(MIN_INTERVAL)
        ));
    }

    Ok(interval)
}

fn parse_seconds(seconds: &str, original: &str) -> Result<u64, String> {
    if seconds.is_empty() {
        return Ok(0);
    }

    if !seconds.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("invalid interval '{original}'"));
    }

    seconds
        .parse()
        .map_err(|_| format!("invalid interval '{original}'"))
}

fn format_interval(interval: Duration) -> String {
    let seconds = interval.as_secs();
    let nanos = interval.subsec_nanos();

    if nanos == 0 {
        return format!("{seconds}.0");
    }

    let mut fraction = format!("{nanos:09}");
    while fraction.ends_with('0') {
        fraction.pop();
    }

    format!("{seconds}.{fraction}")
}

fn clear_screen() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[H\x1b[2J\x1b[3J")?;
    stdout.flush()
}

fn terminal_size(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn print_output(output: &Output, rows: usize, columns: usize) {
    let mut rows_left = rows;
    print_text(
        &String::from_utf8_lossy(&output.stdout),
        &mut rows_left,
        columns,
    );
    print_text(
        &String::from_utf8_lossy(&output.stderr),
        &mut rows_left,
        columns,
    );

    if !output.status.success() && rows_left > 0 {
        println!("watch: command exited with {}", output.status);
    }
}

fn print_text(text: &str, rows_left: &mut usize, columns: usize) {
    if *rows_left == 0 {
        return;
    }

    for line in text.lines() {
        if *rows_left == 0 {
            break;
        }

        println!("{}", truncate_line(line, columns));
        *rows_left -= 1;
    }
}

fn truncate_line(line: &str, columns: usize) -> &str {
    if line.len() <= columns {
        return line;
    }

    let mut end = 0;
    for (index, _) in line.char_indices() {
        if index > columns {
            break;
        }
        end = index;
    }

    &line[..end]
}

fn print_usage() {
    println!("usage: watch [-n SECONDS] [-t] [--no-clear] [--] COMMAND [ARG]...");
}
