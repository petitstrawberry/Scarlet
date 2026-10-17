use scarlet_os::socket::Socket;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream, UdpSocket};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Instant;

#[path = "resolverd/dns.rs"]
mod dns;
use dns::{Answer, Cache, ResolverConfig};

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
use std::thread;
use std::time::Duration;

const SOCKET_PATH: &str = "/tmp/resolverd.sock";
const RESOLV_CONF: &str = "/etc/resolv.conf";
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
    if let Err(err) = server.listen(64) {
        println!("[resolverd] listen failed: {err:?}");
        return ExitCode::from(1);
    }

    println!("[resolverd] listening at {SOCKET_PATH}");

    let (jobs, incoming) = mpsc::sync_channel::<Socket>(64);
    let incoming = Arc::new(Mutex::new(incoming));
    for index in 0..8 {
        let incoming = Arc::clone(&incoming);
        if let Err(error) = thread::Builder::new()
            .name(format!("resolver-request-{index}"))
            .spawn(move || {
                loop {
                    let client = incoming.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    let Ok(client) = client else {
                        break;
                    };
                    if let Err(error) = handle_client(client) {
                        println!("[resolverd] request failed: {error}");
                    }
                }
            })
        {
            println!("[resolverd] failed to start request worker: {error}");
            return ExitCode::from(1);
        }
    }

    loop {
        let client = match server.accept() {
            Ok(client) => client,
            Err(err) => {
                println!("[resolverd] accept failed: {err:?}");
                continue;
            }
        };

        if jobs.send(client).is_err() {
            return ExitCode::from(1);
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
            Err(err) => {
                eprintln!("[resolverd] lookup {host} failed: {err}");
                format!("ERR {err}\n")
            }
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

    let config = read_resolver_config()?;
    if config.nameservers.is_empty() {
        return Err("no DNS servers are configured".to_owned());
    }

    let absolute_host = host.trim_end_matches('.');
    let mut candidates = Vec::new();
    if !host.ends_with('.') && !absolute_host.contains('.') {
        for domain in &config.search_domains {
            let candidate = format!("{absolute_host}.{domain}");
            if is_valid_hostname(&candidate) && !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates.push(absolute_host.to_owned());

    let mut last_error = String::from("all configured DNS servers failed");
    for candidate in candidates {
        let cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
        if let Some(addresses) =
            cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&config, &candidate, Instant::now())
        {
            return Ok(addresses);
        }
        let query = build_query(&candidate)?;
        for nameserver in &config.nameservers {
            match query_nameserver(&query, *nameserver, &candidate) {
                Ok(answer) if !answer.addresses.is_empty() => {
                    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(
                        &config,
                        &candidate,
                        &answer,
                        Instant::now(),
                    );
                    return Ok(answer.addresses);
                }
                Ok(_) => last_error = format!("no A records for {candidate}"),
                Err(error) => last_error = error,
            }
        }
    }
    Err(last_error)
}

fn query_nameserver(query: &[u8], nameserver: Ipv4Addr, host: &str) -> Result<Answer, String> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|err| format!("udp bind failed: {err}"))?;
    socket
        .set_read_timeout(Some(DNS_TIMEOUT))
        .map_err(|err| format!("failed to set DNS timeout: {err}"))?;
    socket
        .connect(SocketAddrV4::new(nameserver, DNS_PORT))
        .map_err(|err| format!("dns connect failed: {err}"))?;
    socket
        .send(query)
        .map_err(|err| format!("dns send to {nameserver} failed: {err}"))?;

    let mut response = [0; MAX_DNS_PACKET];
    let length = socket
        .recv(&mut response)
        .map_err(|err| format!("dns receive from {nameserver} failed: {err}"))?;
    let id = u16::from_be_bytes([query[0], query[1]]);
    if length >= 4 && response[2] & 2 != 0 {
        // Classic UDP DNS has a 512-byte limit. Follow a truncated answer over
        // TCP rather than caching a partial address/alias set.
        let address = std::net::SocketAddr::V4(SocketAddrV4::new(nameserver, DNS_PORT));
        let mut stream = TcpStream::connect_timeout(&address, DNS_TIMEOUT)
            .map_err(|error| format!("DNS TCP connect failed: {error}"))?;
        stream
            .set_read_timeout(Some(DNS_TIMEOUT))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(DNS_TIMEOUT))
            .map_err(|error| error.to_string())?;
        stream
            .write_all(&(query.len() as u16).to_be_bytes())
            .map_err(|error| error.to_string())?;
        stream.write_all(query).map_err(|error| error.to_string())?;
        let mut size = [0; 2];
        stream
            .read_exact(&mut size)
            .map_err(|error| error.to_string())?;
        let mut packet = vec![0; u16::from_be_bytes(size) as usize];
        stream
            .read_exact(&mut packet)
            .map_err(|error| error.to_string())?;
        dns::parse_a_response(&packet, host, id)
    } else {
        dns::parse_a_response(&response[..length], host, id)
    }
}

fn read_resolver_config() -> Result<ResolverConfig, String> {
    let content = fs::read_to_string(RESOLV_CONF)
        .map_err(|err| format!("failed to read {RESOLV_CONF}: {err}"))?;
    let mut nameservers = Vec::new();
    let mut search_domains = Vec::new();
    for line in content.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("nameserver") => {
                if let Some(value) = parts.next()
                    && let Ok(address) = value.parse()
                    && !nameservers.contains(&address)
                {
                    nameservers.push(address);
                }
            }
            Some("search") => {
                for domain in parts {
                    let domain = domain.trim_end_matches('.');
                    if is_valid_hostname(domain)
                        && !search_domains.iter().any(|value| value == domain)
                    {
                        search_domains.push(domain.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(ResolverConfig {
        nameservers,
        search_domains,
    })
}

fn build_query(host: &str) -> Result<Vec<u8>, String> {
    let mut query = Vec::with_capacity(12 + host.len() + 6);
    static NEXT_ID: AtomicU16 = AtomicU16::new(0x5343);
    query.extend_from_slice(&NEXT_ID.fetch_add(1, Ordering::Relaxed).to_be_bytes());
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
