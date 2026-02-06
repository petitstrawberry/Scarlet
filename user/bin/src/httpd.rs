// Simple HTTP server - listens for connections and serves files
// Based on tcp_server - identical structure, different response
// Usage: httpd <port>
// Example: httpd 8080

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::fs::File;
use std::io::{Read, SeekFrom, Write};
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};
use std::string::{String, ToString};
use std::vec::Vec;

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    let args = env::args_vec();

    if args.len() != 2 {
        println!("Usage: httpd <port>");
        println!("Example: httpd 8080");
        return 1;
    }

    // Parse port
    let port = match parse_port(&args[1]) {
        Some(p) => p,
        None => {
            println!("[httpd] Invalid port number");
            return 1;
        }
    };

    println!("[httpd] Starting HTTP server on port {}...", port);

    // Create TCP socket
    let listen_socket =
        match Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
        {
            Ok(s) => s,
            Err(_) => {
                println!("[httpd] Failed to create socket");
                return 1;
            }
        };

    // Bind to port
    let bind_addr = Inet4SocketAddress::new([0, 0, 0, 0], port);
    if let Err(_e) = listen_socket.bind_inet(bind_addr) {
        println!("[httpd] Bind failed");
        return 1;
    }

    // Start listening
    if let Err(e) = listen_socket.listen(5) {
        println!("[httpd] Listen failed: {:?}", e);
        return 1;
    }

    println!("[httpd] Listening on port {}", port);

    loop {
        let client_socket = match listen_socket.accept() {
            Ok(s) => s,
            Err(e) => {
                println!("[httpd] Accept failed: {:?}", e);
                continue;
            }
        };

        println!("[httpd] Client connected!");
        handle_client(client_socket);
    }
}

fn parse_port(s: &str) -> Option<u16> {
    let mut result = 0u32;

    for c in s.bytes() {
        match c {
            b'0'..=b'9' => {
                result = result * 10 + (c - b'0') as u32;
                if result > 65535 {
                    return None;
                }
            }
            _ => return None,
        }
    }

    if result == 0 {
        return None;
    }
    Some(result as u16)
}

fn handle_client(mut client_socket: Socket) {
    let mut buf = [0u8; 1024];
    let n = match client_socket.read(&mut buf) {
        Ok(0) => {
            println!("[httpd] Client disconnected immediately");
            return;
        }
        Ok(n) => n,
        Err(_) => {
            println!("[httpd] Receive error");
            return;
        }
    };

    println!("[httpd] Received {} bytes", n);
    let request = match core::str::from_utf8(&buf[..n]) {
        Ok(s) => s,
        Err(_) => {
            println!("[httpd] Invalid UTF-8");
            return;
        }
    };

    let lines: Vec<&str> = request.lines().collect();
    if lines.is_empty() {
        return;
    }

    let parts: Vec<&str> = lines[0].split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }

    let method = parts[0];
    let path = parts[1];

    if method != "GET" {
        let response = "HTTP/1.1 405 Method Not Allowed\r\nAllow: GET\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
        let _ = client_socket.write(response.as_bytes());
        return;
    }

    println!("[httpd] GET {}", path);
    match build_file_headers(path) {
        Some((headers, file_path, file_size)) => {
            if write_all_socket(&mut client_socket, headers.as_bytes()).is_err() {
                println!("[httpd] Send headers failed");
                return;
            }
            stream_file(&mut client_socket, &file_path, file_size);
        }
        None => {
            send_not_found(&mut client_socket);
        }
    }

    println!("[httpd] Response sent, closing connection");
    let _ = client_socket.shutdown(std::socket::ShutdownHow::Both);
}

fn build_file_headers(path: &str) -> Option<(String, String, u64)> {
    let file_path = sanitize_path(path)?;
    let file_size = file_size(&file_path)?;
    let content_type = content_type(&file_path);
    let mut headers = String::new();
    headers.push_str("HTTP/1.1 200 OK\r\n");
    headers.push_str("Content-Type: ");
    headers.push_str(content_type);
    headers.push_str("\r\nConnection: close\r\nContent-Length: ");
    headers.push_str(&file_size.to_string());
    headers.push_str("\r\n\r\n");
    Some((headers, file_path, file_size))
}

fn stream_file(client_socket: &mut Socket, file_path: &str, file_size: u64) {
    let mut file = match File::open(file_path) {
        Ok(f) => f,
        Err(_) => {
            println!("[httpd] Failed to open file for streaming: {}", file_path);
            return;
        }
    };

    let mut chunk = [0u8; 512];
    let mut remaining = file_size as usize;
    loop {
        if remaining == 0 {
            break;
        }
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let to_send = if n > remaining { remaining } else { n };
                if write_all_socket(client_socket, &chunk[..to_send]).is_err() {
                    println!("[httpd] Send failed");
                    break;
                }
                remaining -= to_send;
            }
            Err(_) => break,
        }
    }
}

fn send_not_found(client_socket: &mut Socket) {
    let body = b"Not Found";
    let response =
        String::from("HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: 9\r\n\r\n");
    let _ = client_socket.write(response.as_bytes());
    let _ = client_socket.write(body);
}

fn write_all_socket(client_socket: &mut Socket, mut buf: &[u8]) -> Result<(), ()> {
    while !buf.is_empty() {
        match client_socket.write(buf) {
            Ok(0) | Err(_) => return Err(()),
            Ok(n) => buf = &buf[n..],
        }
    }
    Ok(())
}

fn file_size(file_path: &str) -> Option<u64> {
    let mut file = File::open(file_path).ok()?;
    let size = file.seek(SeekFrom::End(0)).ok()?;
    let _ = file.seek(SeekFrom::Start(0));
    Some(size)
}

fn sanitize_path(request_path: &str) -> Option<String> {
    let path = match request_path.split('?').next() {
        Some(p) => p,
        None => return None,
    };

    if !path.starts_with('/') {
        return None;
    }

    let mut full = String::from("/www");
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
