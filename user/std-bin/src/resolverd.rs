use scarlet_os::socket::Socket;
use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

const SOCKET_PATH: &str = "/tmp/resolverd.sock";
const RESOLV_CONF: &str = "/etc/resolv.conf";
const FALLBACK_DNS: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
const DNS_PORT: u16 = 53;
const MAX_DNS_PACKET: usize = 512;
const DNS_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    println!("[resolverd] starting");

    let server = match Socket::new() {
        Ok(socket) => socket,
        Err(err) => {
            println!("[resolverd] socket create failed: {err:?}");
            return ExitCode::from(1);
        }
    };

    if let Err(err) = server.bind(SOCKET_PATH) {
        println!("[resolverd] bind {SOCKET_PATH} failed: {err:?}");
        return ExitCode::from(1);
    }
    if let Err(err) = server.listen(16) {
        println!("[resolverd] listen failed: {err:?}");
        return ExitCode::from(1);
    }

    println!("[resolverd] listening at {SOCKET_PATH}");

    loop {
        let client = match server.accept() {
            Ok(client) => client,
            Err(err) => {
                println!("[resolverd] accept failed: {err:?}");
                continue;
            }
        };

        let spawn_result = thread::Builder::new()
            .name(String::from("resolver-request"))
            .spawn(move || {
                if let Err(err) = handle_client(client) {
                    println!("[resolverd] request failed: {err}");
                }
            });
        if let Err(err) = spawn_result {
            println!("[resolverd] failed to start request worker: {err}");
        }
    }
}

fn handle_client(client: Socket) -> Result<(), String> {
    let stream = client
        .as_stream()
        .map_err(|err| format!("client is not a stream: {err:?}"))?;
    let mut request = [0; 512];
    let n = stream
        .read(&mut request)
        .map_err(|err| format!("read failed: {err:?}"))?;
    if n == 0 {
        return Ok(());
    }

    let line = std::str::from_utf8(&request[..n])
        .map_err(|_| "request is not UTF-8".to_owned())?
        .trim();

    let response = match parse_request(line) {
        Ok(host) => match lookup_a(host) {
            Ok(addrs) if !addrs.is_empty() => {
                let mut response = String::from("OK");
                for addr in addrs {
                    response.push(' ');
                    response.push_str(&addr.to_string());
                }
                response.push('\n');
                response
            }
            Ok(_) => String::from("ERR no A records\n"),
            Err(err) => format!("ERR {err}\n"),
        },
        Err(err) => format!("ERR {err}\n"),
    };

    stream
        .write_all(response.as_bytes())
        .map_err(|err| format!("write failed: {err:?}"))?;
    Ok(())
}

fn parse_request(line: &str) -> Result<&str, &'static str> {
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some("A"), Some(host), None) if is_valid_hostname(host) => Ok(host),
        (Some("A"), Some(_), None) => Err("invalid hostname"),
        _ => Err("expected: A <hostname>"),
    }
}

fn lookup_a(host: &str) -> Result<Vec<Ipv4Addr>, String> {
    if let Ok(addr) = host.parse::<Ipv4Addr>() {
        return Ok(vec![addr]);
    }

    let nameserver = read_nameserver().unwrap_or(FALLBACK_DNS);
    let query = build_query(host)?;
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|err| format!("udp bind failed: {err}"))?;
    socket
        .set_read_timeout(Some(DNS_TIMEOUT))
        .map_err(|err| format!("failed to set DNS timeout: {err}"))?;
    let dns_addr = SocketAddrV4::new(nameserver, DNS_PORT);

    socket
        .send_to(&query, dns_addr)
        .map_err(|err| format!("dns send failed: {err}"))?;

    let mut response = [0; MAX_DNS_PACKET];
    let (n, _) = socket
        .recv_from(&mut response)
        .map_err(|err| format!("dns receive failed: {err}"))?;

    parse_a_response(&response[..n])
}

fn read_nameserver() -> Option<Ipv4Addr> {
    let content = fs::read_to_string(RESOLV_CONF).ok()?;
    for line in content.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        if parts.next() == Some("nameserver")
            && let Some(value) = parts.next()
            && let Ok(addr) = value.parse()
        {
            return Some(addr);
        }
    }
    None
}

fn build_query(host: &str) -> Result<Vec<u8>, String> {
    let mut query = Vec::with_capacity(12 + host.len() + 6);
    query.extend_from_slice(&0x5343u16.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());

    for label in host.trim_end_matches('.').split('.') {
        let len = label.len();
        if len == 0 || len > 63 {
            return Err("invalid hostname label".to_owned());
        }
        query.push(len as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&1u16.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());

    Ok(query)
}

fn parse_a_response(packet: &[u8]) -> Result<Vec<Ipv4Addr>, String> {
    if packet.len() < 12 {
        return Err("short DNS response".to_owned());
    }
    if u16::from_be_bytes([packet[0], packet[1]]) != 0x5343 {
        return Err("DNS response id mismatch".to_owned());
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x8000 == 0 {
        return Err("not a DNS response".to_owned());
    }
    let rcode = flags & 0x000f;
    if rcode != 0 {
        return Err(format!("DNS error rcode={rcode}"));
    }

    let qdcount = read_u16(packet, 4)?;
    let ancount = read_u16(packet, 6)?;
    let mut offset = 12usize;

    for _ in 0..qdcount {
        offset = skip_name(packet, offset)?;
        offset = offset.checked_add(4).ok_or("DNS offset overflow")?;
        if offset > packet.len() {
            return Err("truncated DNS question".to_owned());
        }
    }

    let mut addrs = Vec::new();
    for _ in 0..ancount {
        offset = skip_name(packet, offset)?;
        if offset + 10 > packet.len() {
            return Err("truncated DNS answer".to_owned());
        }

        let record_type = read_u16(packet, offset)?;
        let class = read_u16(packet, offset + 2)?;
        let rdlen = read_u16(packet, offset + 8)? as usize;
        offset += 10;

        if offset + rdlen > packet.len() {
            return Err("truncated DNS rdata".to_owned());
        }

        if record_type == 1 && class == 1 && rdlen == 4 {
            addrs.push(Ipv4Addr::new(
                packet[offset],
                packet[offset + 1],
                packet[offset + 2],
                packet[offset + 3],
            ));
        }
        offset += rdlen;
    }

    Ok(addrs)
}

fn skip_name(packet: &[u8], mut offset: usize) -> Result<usize, String> {
    loop {
        let Some(&len) = packet.get(offset) else {
            return Err("truncated DNS name".to_owned());
        };
        if len & 0xc0 == 0xc0 {
            if offset + 1 >= packet.len() {
                return Err("truncated DNS compression pointer".to_owned());
            }
            return Ok(offset + 2);
        }
        if len == 0 {
            return Ok(offset + 1);
        }
        if len & 0xc0 != 0 {
            return Err("unsupported DNS label".to_owned());
        }
        offset = offset
            .checked_add(1 + len as usize)
            .ok_or_else(|| "DNS name offset overflow".to_owned())?;
        if offset > packet.len() {
            return Err("truncated DNS label".to_owned());
        }
    }
}

fn read_u16(packet: &[u8], offset: usize) -> Result<u16, String> {
    if offset + 2 > packet.len() {
        return Err("truncated DNS integer".to_owned());
    }
    Ok(u16::from_be_bytes([packet[offset], packet[offset + 1]]))
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
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            return false;
        }
    }

    true
}
