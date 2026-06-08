use std::env;
use std::error::Error;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process::ExitCode;

use url::Url;

const DEFAULT_URL: &str = "http://10.0.2.2:8080/";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            println!("[http-get-demo] {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let url = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_URL.to_owned())
        .parse::<Url>()?;

    if url.scheme() != "http" {
        return Err("only http:// URLs are supported".into());
    }

    let host = url.host_str().ok_or("URL is missing a host")?;
    let port = url.port_or_known_default().ok_or("URL is missing a port")?;
    let addr = resolve_addr(host, port)?;

    println!("[http-get-demo] GET {url}");
    println!("[http-get-demo] connecting to {addr}");

    let mut stream = TcpStream::connect(addr)?;
    let request = build_request(&url, host);
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    print_summary(&response)?;

    Ok(())
}

fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr, Box<dyn Error>> {
    let mut addrs = (host, port).to_socket_addrs()?;
    addrs
        .find(|addr| addr.is_ipv4())
        .or_else(|| addrs.next())
        .ok_or_else(|| format!("no address resolved for {host}:{port}").into())
}

fn build_request(url: &Url, host: &str) -> String {
    let mut path = url.path().to_owned();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }

    format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: scarlet-http-get-demo/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    )
}

fn print_summary(response: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Response::new(&mut headers);
    let body_offset = match parsed.parse(response)? {
        httparse::Status::Complete(offset) => offset,
        httparse::Status::Partial => return Err("received partial HTTP response".into()),
    };

    println!(
        "[http-get-demo] status={} reason={}",
        parsed.code.unwrap_or(0),
        parsed.reason.unwrap_or("")
    );

    for header in parsed.headers.iter() {
        let value = String::from_utf8_lossy(header.value);
        println!("[http-get-demo] header {}: {value}", header.name);
    }

    let body = &response[body_offset..];
    println!("[http-get-demo] body-bytes={}", body.len());
    if !body.is_empty() {
        let preview_len = body.len().min(256);
        let preview = String::from_utf8_lossy(&body[..preview_len]);
        println!("[http-get-demo] body-preview:");
        println!("{preview}");
    }

    Ok(())
}
