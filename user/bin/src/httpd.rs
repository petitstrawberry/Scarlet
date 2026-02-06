// Simple HTTP server - listens for connections and serves files
// Based on tcp_server - identical structure, different response
// Usage: httpd <port>
// Example: httpd 8080

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};
use std::string::{String, ToString};
use std::thread;
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
        thread::spawn(move || handle_client(client_socket));
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
    match client_socket.read(&mut buf) {
        Ok(0) => {
            println!("[httpd] Client disconnected");
        }
        Ok(n) => {
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

            let first_line = lines[0];
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            if parts.len() < 2 {
                return;
            }

            let method = parts[0];
            let path = parts[1];

            if method != "GET" {
                let response = "HTTP/1.1 405 Method Not Allowed\r\nAllow: GET\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
                let _ = client_socket.write(response.as_bytes());
                let _ = client_socket.shutdown(std::socket::ShutdownHow::Both);
                return;
            }

            println!("[httpd] GET {}", path);
            match build_file_response(path) {
                Some((headers, body)) => {
                    if client_socket.write(headers.as_bytes()).is_err()
                        || client_socket.write(&body).is_err()
                    {
                        println!("[httpd] Send failed");
                    }
                }
                None => {
                    let body = b"Not Found";
                    let response = "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: 9\r\n\r\n";
                    let _ = client_socket.write(response.as_bytes());
                    let _ = client_socket.write(body);
                }
            }
            let _ = client_socket.shutdown(std::socket::ShutdownHow::Both);
        }
        Err(_) => {
            println!("[httpd] Receive error");
        }
    }
}

fn build_file_response(path: &str) -> Option<(String, Vec<u8>)> {
    let file_path = sanitize_path(path)?;
    let mut file = File::open(&file_path).ok()?;
    let mut body = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => return None,
        }
    }

    let content_type = content_type(&file_path);
    let mut headers = String::new();
    headers.push_str("HTTP/1.1 200 OK\r\n");
    headers.push_str("Content-Type: ");
    headers.push_str(content_type);
    headers.push_str("\r\nConnection: close\r\nContent-Length: ");
    headers.push_str(&body.len().to_string());
    headers.push_str("\r\n\r\n");
    Some((headers, body))
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
