//! Query and follow records collected by Scarlet's `logd` daemon.

use log_protocol::{ANY_PID, ANY_PRIORITY, LogPriority};
use std::string::String;

#[cfg(test)]
use std::vec::Vec;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    unit: String,
    follow: bool,
    lines: u32,
    pid: i32,
    max_priority: u8,
    after_sequence: u64,
    show_help: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            unit: String::new(),
            follow: false,
            lines: 100,
            pid: ANY_PID,
            max_priority: ANY_PRIORITY,
            after_sequence: 0,
            show_help: false,
        }
    }
}

fn parse_options(arguments: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-u" | "--unit" => {
                options.unit = arguments
                    .next()
                    .ok_or_else(|| String::from("missing value for --unit"))?;
            }
            "-f" | "--follow" => options.follow = true,
            "-n" | "--lines" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| String::from("missing value for --lines"))?;
                options.lines = if value == "all" {
                    0
                } else {
                    value
                        .parse::<u32>()
                        .map_err(|_| String::from("invalid value for --lines"))?
                };
            }
            "-p" | "--priority" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| String::from("missing value for --priority"))?;
                let value = value.to_ascii_lowercase();
                options.max_priority = LogPriority::parse(&value)
                    .ok_or_else(|| String::from("invalid value for --priority"))?
                    .as_u8();
            }
            "--pid" => {
                options.pid = arguments
                    .next()
                    .ok_or_else(|| String::from("missing value for --pid"))?
                    .parse::<i32>()
                    .map_err(|_| String::from("invalid value for --pid"))?;
                if options.pid < 0 {
                    return Err(String::from("--pid must be non-negative"));
                }
            }
            "--after-sequence" => {
                options.after_sequence = arguments
                    .next()
                    .ok_or_else(|| String::from("missing value for --after-sequence"))?
                    .parse::<u64>()
                    .map_err(|_| String::from("invalid value for --after-sequence"))?;
            }
            "-b" | "--boot" => {
                // logd currently retains only the active in-memory boot journal.
            }
            "-h" | "--help" => options.show_help = true,
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(options)
}

fn print_help() {
    println!("Usage: logctl [OPTIONS]");
    println!();
    println!("Options:");
    println!("  -u, --unit UNIT          Show one service or application unit");
    println!("  -f, --follow             Follow new matching records");
    println!("  -n, --lines COUNT|all    Show newest records (default: 100)");
    println!("  -p, --priority LEVEL     Show LEVEL and more important records");
    println!("      --pid PID            Show one process ID");
    println!("      --after-sequence N   Show records newer than sequence N");
    println!("  -b, --boot               Select the active in-memory boot journal");
    println!("  -h, --help               Show this help");
}

#[cfg(target_os = "scarlet")]
mod runtime {
    use super::Options;
    use log_protocol::{
        HEADER_SIZE, Header, LogRecord, MAX_PAYLOAD_SIZE, MSG_ERROR, MSG_QUERY, MSG_QUERY_END,
        MSG_RECORD, Query, SOCKET_PATH,
    };
    use scarlet_os::socket::Socket;
    use std::io::{Read, Write};
    use std::process::ExitCode;
    use std::vec::Vec;

    pub(super) fn run(options: Options) -> ExitCode {
        let mut socket = match Socket::new() {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("logctl: failed to create socket: {error:?}");
                return ExitCode::from(1);
            }
        };
        if let Err(error) = socket.connect(SOCKET_PATH) {
            eprintln!("logctl: cannot connect to logd: {error:?}");
            return ExitCode::from(1);
        }

        let query = Query {
            after_sequence: options.after_sequence,
            tail: options.lines,
            follow: options.follow,
            unit: options.unit,
            pid: options.pid,
            max_priority: options.max_priority,
        };
        let payload = match query.to_payload() {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("logctl: invalid query: {error:?}");
                return ExitCode::from(2);
            }
        };
        if let Err(error) = write_frame(&mut socket, MSG_QUERY, &payload) {
            eprintln!("logctl: failed to send query: {error}");
            return ExitCode::from(1);
        }

        loop {
            let Some((header, payload)) = read_frame(&mut socket) else {
                if options.follow {
                    eprintln!("logctl: logd connection closed");
                    return ExitCode::from(1);
                }
                return ExitCode::SUCCESS;
            };
            match header.message_type {
                MSG_RECORD => match LogRecord::from_payload(&payload) {
                    Ok(record) => print_record(&record),
                    Err(error) => {
                        eprintln!("logctl: invalid record from logd: {error:?}");
                        return ExitCode::from(1);
                    }
                },
                MSG_QUERY_END => return ExitCode::SUCCESS,
                MSG_ERROR => {
                    eprintln!("logctl: {}", String::from_utf8_lossy(&payload));
                    return ExitCode::from(1);
                }
                message_type => {
                    eprintln!("logctl: unexpected response type {message_type:#x}");
                    return ExitCode::from(1);
                }
            }
        }
    }

    fn print_record(record: &LogRecord) {
        let seconds = record.monotonic_ns / 1_000_000_000;
        let micros = record.monotonic_ns % 1_000_000_000 / 1_000;
        let message = String::from_utf8_lossy(&record.message);
        if record.stream == log_protocol::LogStream::Stderr {
            println!(
                "[{seconds:>6}.{micros:06}] {}[{}] {}: {}",
                record.unit,
                record.pid,
                record.priority.as_str(),
                message
            );
        } else {
            println!(
                "[{seconds:>6}.{micros:06}] {}[{}]: {}",
                record.unit, record.pid, message
            );
        }
        let _ = std::io::stdout().flush();
    }

    fn read_frame(stream: &mut Socket) -> Option<(Header, Vec<u8>)> {
        let mut header_bytes = [0u8; HEADER_SIZE];
        stream.read_exact(&mut header_bytes).ok()?;
        let header = Header::from_le_bytes(header_bytes);
        let payload_size = header.payload_size as usize;
        if payload_size > MAX_PAYLOAD_SIZE {
            return None;
        }
        let mut payload = vec![0u8; payload_size];
        stream.read_exact(&mut payload).ok()?;
        Some((header, payload))
    }

    fn write_frame(stream: &mut Socket, message_type: u32, payload: &[u8]) -> std::io::Result<()> {
        let header = Header {
            message_type,
            payload_size: payload.len() as u32,
        };
        stream.write_all(&header.to_le_bytes())?;
        stream.write_all(payload)
    }
}

#[cfg(target_os = "scarlet")]
fn main() -> std::process::ExitCode {
    let options = match parse_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("logctl: {error}");
            eprintln!("Try 'logctl --help'.");
            return std::process::ExitCode::from(2);
        }
    };
    if options.show_help {
        print_help();
        return std::process::ExitCode::SUCCESS;
    }
    runtime::run(options)
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    match parse_options(std::env::args().skip(1)) {
        Ok(options) if options.show_help => print_help(),
        Ok(_) => eprintln!("logctl is only available on Scarlet OS"),
        Err(error) => eprintln!("logctl: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| String::from(*value)).collect()
    }

    #[test]
    fn parses_unit_follow_tail_priority_and_pid() {
        let options = parse_options(args(&[
            "-u", "sws", "-f", "-n", "200", "-p", "warning", "--pid", "42",
        ]))
        .unwrap();
        assert_eq!(options.unit, "sws");
        assert!(options.follow);
        assert_eq!(options.lines, 200);
        assert_eq!(options.max_priority, LogPriority::Warning.as_u8());
        assert_eq!(options.pid, 42);
    }

    #[test]
    fn all_lines_and_boot_alias_are_accepted() {
        let options = parse_options(args(&["-n", "all", "-b"])).unwrap();
        assert_eq!(options.lines, 0);
    }

    #[test]
    fn invalid_priority_is_rejected() {
        assert_eq!(
            parse_options(args(&["-p", "verbose"])),
            Err(String::from("invalid value for --priority"))
        );
    }
}
