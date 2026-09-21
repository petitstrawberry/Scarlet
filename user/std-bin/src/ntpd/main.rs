//! Leaf SNTP client. The network exchange is unprivileged; stemd applies UTC.

mod protocol;
#[allow(dead_code)]
#[path = "../stemd/protocol.rs"]
mod stemd_protocol;

use clap::Parser;
use scarlet_os::time::monotonic_time_ns;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(about = "Synchronize Scarlet wall time using SNTP (without writing the RTC)")]
struct Args {
    #[arg(long, default_value = "time.cloudflare.com")]
    server: String,
    /// Seconds between successful synchronizations; failures use backoff.
    #[arg(long, default_value_t = 1024, value_parser = clap::value_parser!(u64).range(15..=86400))]
    interval: u64,
    /// Synchronize once and exit with success/failure status.
    #[arg(long)]
    once: bool,
    /// Query once and print the sample without changing the clock.
    #[arg(long)]
    query: bool,
}

#[derive(Debug)]
enum Failure {
    Retry(String),
    Kiss {
        code: [u8; 4],
        retry_after_secs: u64,
    },
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Retry(error.to_string())
    }
}

fn cookie() -> [u8; 8] {
    let mut bytes = [0; 8];
    // Correlation only, not authentication. A board without an entropy source
    // can still bootstrap UTC; TLS retains its separate strict RNG policy.
    let count = unsafe {
        scarlet_sys::syscall3(
            scarlet_sys::Syscall::GetRandom,
            bytes.as_mut_ptr() as usize,
            bytes.len(),
            0,
        )
    };
    if count != bytes.len() || bytes == [0; 8] {
        bytes = (monotonic_time_ns() | 1).to_be_bytes();
    }
    bytes
}

fn query(server: &str, attempt: usize) -> Result<(SocketAddr, protocol::Sample), Failure> {
    let mut addresses: Vec<_> = (server, 123)
        .to_socket_addrs()?
        .filter(SocketAddr::is_ipv4)
        .collect();
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(Failure::Retry("server has no IPv4 address".into()));
    }
    // One packet per attempt, rotate addresses after failures. No rapid burst
    // at boot, and DNS is retried as network connectivity becomes available.
    let peer = addresses[attempt % addresses.len()];
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    let cookie = cookie();
    let request = protocol::request(cookie);
    let sent = monotonic_time_ns();
    if socket.send_to(&request, peer)? != request.len() {
        return Err(Failure::Retry("short NTP datagram send".into()));
    }
    let deadline = sent.saturating_add(3_000_000_000);
    let mut packet = [0; 512];
    loop {
        let remaining = deadline.saturating_sub(monotonic_time_ns());
        if remaining == 0 {
            return Err(Failure::Retry("NTP response timed out".into()));
        }
        socket.set_read_timeout(Some(Duration::from_nanos(remaining)))?;
        let (length, source) = socket.recv_from(&mut packet)?;
        let received = monotonic_time_ns();
        if source != peer {
            continue;
        }
        match protocol::response(&packet[..length], cookie, sent, received) {
            Ok(sample) => return Ok((peer, sample)),
            Err(protocol::Error::Invalid(_)) => continue,
            Err(protocol::Error::Kiss {
                code,
                retry_after_secs,
            }) => {
                return Err(Failure::Kiss {
                    code,
                    retry_after_secs,
                });
            }
        }
    }
}

fn apply(sample: &protocol::Sample) -> Result<(), Failure> {
    use scarlet_os::handle::capability::StreamError;
    use scarlet_os::socket::Socket;
    let error = |e| Failure::Retry(format!("stemd socket: {e:?}"));
    let socket = Socket::new().map_err(error)?;
    socket.connect("/tmp/stemd.sock").map_err(error)?;
    socket.set_nonblocking(true).map_err(error)?;
    let stream = socket
        .as_stream()
        .map_err(|e| Failure::Retry(format!("stemd stream: {e:?}")))?;
    let command = stemd_protocol::system_time_command(sample.unix_ns, sample.monotonic_ns);
    let deadline = monotonic_time_ns().saturating_add(3_000_000_000);
    let mut sent = 0;
    while sent < command.len() && monotonic_time_ns() < deadline {
        match stream.write(&command[sent..]) {
            Ok(0) => return Err(Failure::Retry("stemd closed the connection".into())),
            Ok(n) => sent += n,
            Err(StreamError::WouldBlock | StreamError::Interrupted) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(Failure::Retry(format!("stemd write: {e:?}"))),
        }
    }
    let mut reply = [0; 128];
    let mut count = 0;
    while sent == command.len() && count < reply.len() && monotonic_time_ns() < deadline {
        match stream.read(&mut reply[count..]) {
            Ok(0) => break,
            Ok(n) => {
                count += n;
                if reply[..count].contains(&b'\n') {
                    break;
                }
            }
            Err(StreamError::WouldBlock | StreamError::Interrupted) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(Failure::Retry(format!("stemd read: {e:?}"))),
        }
    }
    if &reply[..count] == b"OK: System time updated\n" {
        Ok(())
    } else {
        Err(Failure::Retry(format!(
            "stemd did not apply UTC: {}",
            String::from_utf8_lossy(&reply[..count]).trim()
        )))
    }
}

fn synchronize(args: &Args, attempt: usize) -> Result<(), Failure> {
    let (peer, sample) = query(&args.server, attempt)?;
    if !args.query {
        apply(&sample)?;
    }
    println!(
        "ntpd: {} server={} ({peer}) stratum={} delay_us={} unix_ns={} monotonic_ns={}",
        if args.query { "sample" } else { "synchronized" },
        args.server,
        sample.stratum,
        sample.delay_ns / 1000,
        sample.unix_ns,
        sample.monotonic_ns
    );
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    let mut retry = 16u64;
    let mut retry_floor = 16u64;
    let mut minimum_interval = args.interval;
    let mut attempt = 0usize;
    loop {
        let result = synchronize(&args, attempt);
        let success = result.is_ok();
        let mut denied = false;
        let delay = match result {
            Ok(()) => {
                retry = retry_floor;
                minimum_interval
            }
            Err(Failure::Retry(message)) => {
                if args.once || args.query {
                    eprintln!("ntpd: {message}");
                } else {
                    eprintln!("ntpd: {message}; retry in {retry}s");
                }
                let delay = retry;
                retry = retry.saturating_mul(2).min(minimum_interval.max(16));
                delay
            }
            Err(Failure::Kiss {
                code,
                retry_after_secs,
            }) => {
                denied = code == *b"DENY" || code == *b"RSTR";
                minimum_interval = minimum_interval
                    .saturating_mul(2)
                    .max(retry_after_secs)
                    .min(131072);
                retry = minimum_interval;
                retry_floor = minimum_interval;
                eprintln!(
                    "ntpd: server KoD {}: {}",
                    String::from_utf8_lossy(&code),
                    if denied {
                        "requests disabled until restart"
                    } else {
                        "increasing poll interval"
                    }
                );
                minimum_interval
            }
        };
        if args.once || args.query {
            return if success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        // Do not let a service restart loop turn DENY/RSTR into repeated traffic.
        if denied {
            loop {
                std::thread::sleep(Duration::from_secs(86400));
            }
        }
        std::thread::sleep(Duration::from_secs(delay));
        attempt = attempt.wrapping_add(1);
    }
}
