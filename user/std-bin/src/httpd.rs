use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::process::ExitCode;
use std::thread;

fn main() -> ExitCode {
    println!("[httpd] Rust std version");

    let args = env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        println!("Usage: httpd <port>");
        println!("Example: httpd 8080");
        return ExitCode::from(1);
    }

    let port = match args[1].parse::<u16>() {
        Ok(port) if port != 0 => port,
        _ => {
            println!("[httpd] Invalid port number");
            return ExitCode::from(1);
        }
    };

    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(err) => {
            println!("[httpd] Bind failed: {err}");
            return ExitCode::from(1);
        }
    };

    println!("[httpd] Listening on port {port}");
    loop {
        let (stream, peer) = match listener.accept() {
            Ok(client) => client,
            Err(err) => {
                println!("[httpd] Accept failed: {err}");
                continue;
            }
        };

        println!("[httpd] Client connected from {peer}");
        thread::spawn(move || handle_client(stream));
    }
}

fn handle_client(mut stream: TcpStream) {
    let mut buf = [0; 1024];
    let n = match stream.read(&mut buf) {
        Ok(0) => {
            println!("[httpd] Client disconnected immediately");
            return;
        }
        Ok(n) => n,
        Err(err) => {
            println!("[httpd] Receive error: {err}");
            return;
        }
    };

    println!("[httpd] Received {n} bytes");
    let request = match std::str::from_utf8(&buf[..n]) {
        Ok(request) => request,
        Err(err) => {
            println!("[httpd] Invalid UTF-8 request: {err}");
            return;
        }
    };

    let Some(request_line) = request.lines().next() else {
        return;
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    if method != "GET" {
        let response = "HTTP/1.1 405 Method Not Allowed\r\nAllow: GET\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(response.as_bytes());
        return;
    }

    println!("[httpd] GET {path}");
    match build_file_headers(path) {
        Some((headers, file_path, file_size)) => {
            if stream.write_all(headers.as_bytes()).is_err() {
                println!("[httpd] Send headers failed");
                return;
            }
            stream_file(&mut stream, &file_path, file_size);
        }
        None => send_not_found(&mut stream),
    }

    println!("[httpd] Response sent");
}

fn build_file_headers(path: &str) -> Option<(String, String, u64)> {
    let file_path = sanitize_path(path)?;
    let file_size = file_size(&file_path)?;
    let content_type = content_type(&file_path);
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\nContent-Length: {file_size}\r\n\r\n"
    );
    Some((headers, file_path, file_size))
}

fn stream_file(stream: &mut TcpStream, file_path: &str, file_size: u64) {
    let mut file = match File::open(file_path) {
        Ok(file) => file,
        Err(err) => {
            println!("[httpd] Failed to open {file_path}: {err}");
            return;
        }
    };

    let mut remaining = file_size as usize;
    let mut chunk = [0; 512];
    while remaining > 0 {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let to_send = n.min(remaining);
                if stream.write_all(&chunk[..to_send]).is_err() {
                    println!("[httpd] Send failed");
                    break;
                }
                remaining -= to_send;
            }
            Err(err) => {
                println!("[httpd] File read failed: {err}");
                break;
            }
        }
    }
}

fn send_not_found(stream: &mut TcpStream) {
    let body = b"Not Found";
    let response = "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: 9\r\n\r\n";
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
}

fn file_size(file_path: &str) -> Option<u64> {
    let mut file = File::open(file_path).ok()?;
    let size = file.seek(SeekFrom::End(0)).ok()?;
    let _ = file.seek(SeekFrom::Start(0));
    Some(size)
}

fn sanitize_path(request_path: &str) -> Option<String> {
    let path = request_path.split('?').next()?;
    if !path.starts_with('/') {
        return None;
    }

    let mut full = String::from("/var/www/html");
    if path == "/" {
        full.push_str("/index.html");
        return Some(full);
    }

    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." {
            return None;
        }
        full.push('/');
        full.push_str(segment);
    }

    Some(full)
}

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".html") || path.ends_with(".htm") {
        "text/html"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".gif") {
        "image/gif"
    } else if path.ends_with(".txt") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}
