use scarlet_os::socket::Socket;
use std::io;
use std::net::Ipv4Addr;

pub const RESOLVERD_SOCKET_PATH: &str = "/tmp/resolverd.sock";

pub fn lookup_ipv4(host: &str) -> io::Result<Vec<Ipv4Addr>> {
    if let Ok(addr) = host.parse::<Ipv4Addr>() {
        return Ok(vec![addr]);
    }
    if !is_valid_hostname(host) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid hostname",
        ));
    }

    let socket = Socket::new().map_err(|_| io::Error::other("failed to create resolver socket"))?;
    socket
        .connect(RESOLVERD_SOCKET_PATH)
        .map_err(|_| io::Error::new(io::ErrorKind::ConnectionRefused, "resolverd unavailable"))?;
    let stream = socket
        .as_stream()
        .map_err(|_| io::Error::other("resolver socket is not a stream"))?;

    let request = format!("A {host}\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| io::Error::other(format!("resolver write failed: {err:?}")))?;

    let mut response = Vec::new();
    let mut buf = [0; 256];
    loop {
        let n = stream
            .read(&mut buf)
            .map_err(|err| io::Error::other(format!("resolver read failed: {err:?}")))?;
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buf[..n]);
        if response.contains(&b'\n') || response.len() > 4096 {
            break;
        }
    }

    parse_response(&response)
}

fn parse_response(response: &[u8]) -> io::Result<Vec<Ipv4Addr>> {
    let text = std::str::from_utf8(response)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "resolver response is not UTF-8"))?
        .trim();

    let Some(rest) = text.strip_prefix("OK ") else {
        let message = text.strip_prefix("ERR ").unwrap_or("resolver error");
        return Err(io::Error::other(message.to_owned()));
    };

    let mut addrs = Vec::new();
    for item in rest.split_whitespace() {
        let addr = item.parse::<Ipv4Addr>().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "resolver returned invalid IPv4")
        })?;
        addrs.push(addr);
    }

    if addrs.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "resolver returned no addresses",
        ))
    } else {
        Ok(addrs)
    }
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
