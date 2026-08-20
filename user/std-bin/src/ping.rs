//! Simple ping client for INET.

use scarlet_os::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};
use std::process::ExitCode;
use std::time::Duration;

const RESOLVERD_SOCKET_PATH: &str = "/tmp/resolverd.sock";
const MAX_RESOLVER_RESPONSE: usize = 512;

fn main() -> ExitCode {
    ExitCode::from(run())
}

fn run() -> u8 {
    println!("[ping] start");

    let args: Vec<String> = std::env::args().collect();
    let Some(destination) = args.get(1) else {
        println!("Usage: ping <host>");
        return 1;
    };
    let dest = match resolve_ipv4(destination) {
        Ok(addr) => addr,
        Err(error) => {
            println!("[ping] resolve {destination} failed: {error}");
            return 1;
        }
    };

    let socket = match Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Icmp,
    ) {
        Ok(sock) => sock,
        Err(_) => {
            println!("[ping] socket create failed");
            return 1;
        }
    };

    let dest = Inet4SocketAddress::new(dest, 0);
    let payload = b"scarlet";

    if socket.connect_inet(dest).is_err() {
        println!("[ping] connect failed");
        return 1;
    }

    if let Ok(stream) = socket.as_stream() {
        if stream.write(payload).is_err() {
            println!("[ping] send failed (check netcfg)");
            return 1;
        }
    } else {
        println!("[ping] send failed (no stream)");
        return 1;
    }

    let mut buf = [0u8; 64];
    if socket.set_nonblocking(true).is_err() {
        println!("[ping] recv failed");
        return 1;
    }

    let mut n = 0usize;
    let mut got_reply = false;
    for _ in 0..200 {
        if let Ok(stream) = socket.as_stream() {
            match stream.read(&mut buf) {
                Ok(read) => {
                    n = read;
                    got_reply = true;
                    break;
                }
                Err(_) => {
                    // WouldBlock or no data yet.
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    if !got_reply {
        println!("[ping] timeout");
        return 1;
    }

    if n == payload.len() && &buf[..n] == payload {
        println!("[ping] reply {n} bytes");
        return 0;
    }

    println!("[ping] invalid reply (payload mismatch)");
    1
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
