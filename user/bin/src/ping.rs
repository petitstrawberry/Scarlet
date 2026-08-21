//! Continuous ping client for INET; runs until Ctrl-C.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::sync::atomic::{AtomicBool, Ordering};

use scarlet_os::ipc::{
    EventInfo, event_types, process_control, register_event_handler, unregister_event_handler,
};
use scarlet_os::time::monotonic_time_ns;
use std::env;
use std::format;
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};

const RESOLVERD_SOCKET_PATH: &str = "/tmp/resolverd.sock";
const MAX_RESOLVER_RESPONSE: usize = 512;
const PING_INTERVAL_NS: u64 = 1_000_000_000;
const PING_TIMEOUT_NS: u64 = 1_000_000_000;
const POLL_INTERVAL_MS: u64 = 10;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupt_handler(event_info: &EventInfo) {
    if event_info.content_type == event_types::PROCESS_CONTROL
        && event_info.content_data[0] == process_control::INTERRUPT as u64
    {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }
}

fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("[ping] start");

    let args = env::args_vec();
    let Some(destination) = args.get(1) else {
        println!("Usage: ping <host>");
        return 1;
    };
    let dest = match resolve_ipv4(destination) {
        Ok(addr) => addr,
        Err(error) => {
            println!("[ping] resolve {} failed: {}", destination, error);
            return 1;
        }
    };

    INTERRUPTED.store(false, Ordering::Relaxed);
    if register_event_handler(event_types::PROCESS_CONTROL, interrupt_handler, false).is_err() {
        println!("[ping] failed to register Ctrl-C handler");
        return 1;
    }

    let socket = match Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Icmp,
    ) {
        Ok(sock) => sock,
        Err(_) => {
            println!("[ping] socket create failed");
            let _ = unregister_event_handler(event_types::PROCESS_CONTROL);
            return 1;
        }
    };

    let dest = Inet4SocketAddress::new(dest, 0);
    let payload = b"scarlet";

    if socket.connect_inet(dest).is_err() {
        println!("[ping] connect failed");
        let _ = unregister_event_handler(event_types::PROCESS_CONTROL);
        return 1;
    }

    let mut buf = [0u8; 64];
    if socket.set_nonblocking(true).is_err() {
        println!("[ping] recv failed");
        let _ = unregister_event_handler(event_types::PROCESS_CONTROL);
        return 1;
    }

    let mut transmitted = 0u64;
    let mut received = 0u64;
    let mut min_rtt_ns = u64::MAX;
    let mut max_rtt_ns = 0u64;
    let mut total_rtt_ns = 0u64;
    let mut next_probe_ns = monotonic_time_ns();

    while !interrupted() {
        wait_until(next_probe_ns);
        if interrupted() {
            break;
        }

        let sequence = transmitted.saturating_add(1);
        let sent_at_ns = monotonic_time_ns();
        let sent = if let Ok(stream) = socket.as_stream() {
            stream.write(payload).is_ok()
        } else {
            false
        };
        if !sent {
            println!("[ping] send failed (check netcfg)");
            break;
        }
        transmitted = sequence;

        let timeout_deadline_ns = sent_at_ns.saturating_add(PING_TIMEOUT_NS);
        let mut got_reply = false;
        while !interrupted() && monotonic_time_ns() < timeout_deadline_ns {
            if let Ok(stream) = socket.as_stream() {
                match stream.read(&mut buf) {
                    Ok(read) if read == payload.len() && &buf[..read] == payload => {
                        let rtt_ns = monotonic_time_ns().saturating_sub(sent_at_ns);
                        let rtt_ms = rtt_ns / 1_000_000;
                        let rtt_us = (rtt_ns % 1_000_000) / 1_000;
                        println!(
                            "[ping] reply {} bytes, seq={}, rtt {}.{:03} ms",
                            read, sequence, rtt_ms, rtt_us
                        );
                        received = received.saturating_add(1);
                        min_rtt_ns = min_rtt_ns.min(rtt_ns);
                        max_rtt_ns = max_rtt_ns.max(rtt_ns);
                        total_rtt_ns = total_rtt_ns.saturating_add(rtt_ns);
                        got_reply = true;
                        break;
                    }
                    Ok(_) | Err(_) => {
                        // Ignore unrelated packets and WouldBlock until timeout.
                    }
                }
            }

            std::thread::sleep(core::time::Duration::from_millis(POLL_INTERVAL_MS));
        }

        if interrupted() {
            break;
        }
        if !got_reply {
            println!("[ping] timeout, seq={}", sequence);
        }
        next_probe_ns = sent_at_ns.saturating_add(PING_INTERVAL_NS);
    }

    let _ = unregister_event_handler(event_types::PROCESS_CONTROL);
    print_summary(
        destination,
        transmitted,
        received,
        min_rtt_ns,
        max_rtt_ns,
        total_rtt_ns,
    );
    std::task::exit(if received > 0 { 0 } else { 1 });
}

fn wait_until(deadline_ns: u64) {
    while !interrupted() {
        let remaining_ns = deadline_ns.saturating_sub(monotonic_time_ns());
        if remaining_ns == 0 {
            return;
        }
        let sleep_ms = (remaining_ns / 1_000_000).clamp(1, POLL_INTERVAL_MS);
        std::thread::sleep(core::time::Duration::from_millis(sleep_ms));
    }
}

fn print_summary(
    destination: &str,
    transmitted: u64,
    received: u64,
    min_rtt_ns: u64,
    max_rtt_ns: u64,
    total_rtt_ns: u64,
) {
    println!("\n--- {} ping statistics ---", destination);
    let lost = transmitted.saturating_sub(received);
    let loss_percent = if transmitted == 0 {
        0
    } else {
        lost.saturating_mul(100) / transmitted
    };
    println!(
        "{} packets transmitted, {} packets received, {}% packet loss",
        transmitted, received, loss_percent
    );

    if received > 0 {
        let average_rtt_ns = total_rtt_ns / received;
        let (min_ms, min_us) = split_rtt_ms(min_rtt_ns);
        let (average_ms, average_us) = split_rtt_ms(average_rtt_ns);
        let (max_ms, max_us) = split_rtt_ms(max_rtt_ns);
        println!(
            "rtt min/avg/max = {}.{:03}/{}.{:03}/{}.{:03} ms",
            min_ms, min_us, average_ms, average_us, max_ms, max_us
        );
    }
}

fn split_rtt_ms(rtt_ns: u64) -> (u64, u64) {
    (rtt_ns / 1_000_000, (rtt_ns % 1_000_000) / 1_000)
}

fn parse_ipv4(value: &str) -> Option<[u8; 4]> {
    let mut parts = [0u8; 4];
    let mut index = 0;
    for part in value.split('.') {
        if index >= parts.len() {
            return None;
        }
        parts[index] = part.parse::<u8>().ok()?;
        index += 1;
    }
    if index == parts.len() {
        Some(parts)
    } else {
        None
    }
}

fn resolve_ipv4(value: &str) -> Result<[u8; 4], &'static str> {
    if let Some(addr) = parse_ipv4(value) {
        return Ok(addr);
    }
    if !is_valid_hostname(value) {
        return Err("invalid hostname");
    }

    let socket = Socket::new().map_err(|_| "resolver socket create failed")?;
    socket
        .connect(RESOLVERD_SOCKET_PATH)
        .map_err(|_| "resolverd unavailable")?;
    let stream = socket
        .as_stream()
        .map_err(|_| "resolver socket is not a stream")?;

    let request = format!("A {value}\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "resolver request failed")?;

    let mut response = [0u8; MAX_RESOLVER_RESPONSE];
    let mut response_len = 0usize;
    loop {
        if response_len == response.len() {
            return Err("resolver response too long");
        }
        let read = stream
            .read(&mut response[response_len..])
            .map_err(|_| "resolver response read failed")?;
        if read == 0 {
            break;
        }
        response_len += read;
        if response[..response_len].contains(&b'\n') {
            break;
        }
    }

    parse_resolver_response(&response[..response_len])
}

fn parse_resolver_response(response: &[u8]) -> Result<[u8; 4], &'static str> {
    let text = std::str::from_utf8(response)
        .map_err(|_| "resolver response is not UTF-8")?
        .trim();
    let addresses = text.strip_prefix("OK ").ok_or("resolver lookup failed")?;
    let first = addresses
        .split_whitespace()
        .next()
        .ok_or("resolver returned no IPv4 addresses")?;
    parse_ipv4(first).ok_or("resolver returned an invalid IPv4 address")
}

fn is_valid_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }

    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        let bytes = label.as_bytes();
        if bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-') {
            return false;
        }
        if !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::{is_valid_hostname, parse_resolver_response};

    #[test]
    fn parses_first_resolved_ipv4_address() {
        assert_eq!(
            parse_resolver_response(b"OK 142.250.196.110 142.250.196.78\n"),
            Ok([142, 250, 196, 110])
        );
    }

    #[test]
    fn rejects_invalid_hostname_instead_of_using_default_destination() {
        assert!(!is_valid_hostname("bad..hostname"));
        assert!(!is_valid_hostname("-example.com"));
    }
}
