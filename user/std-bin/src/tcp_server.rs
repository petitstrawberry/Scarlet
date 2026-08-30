use std::env;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        println!("Usage: tcp-server <port>");
        println!("Example: tcp-server 8080");
        return ExitCode::from(1);
    }

    let port = match args[1].parse::<u16>() {
        Ok(port) if port != 0 => port,
        _ => {
            println!("[tcp-server] Invalid port number");
            return ExitCode::from(1);
        }
    };

    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    println!("[tcp-server] Starting on {addr}...");

    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(err) => {
            println!("[tcp-server] Bind failed: {err}");
            return ExitCode::from(1);
        }
    };

    println!("[tcp-server] Listening on port {port}");
    let (mut stream, peer) = match listener.accept() {
        Ok(client) => client,
        Err(err) => {
            println!("[tcp-server] Accept failed: {err}");
            return ExitCode::from(1);
        }
    };

    println!("[tcp-server] Client connected from {peer}");

    let mut buf = [0; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => {
                println!("[tcp-server] Client disconnected");
                break;
            }
            Ok(n) => {
                println!("[tcp-server] Received {n} bytes");
                if let Err(err) = stream.write_all(&buf[..n]) {
                    println!("[tcp-server] Send failed: {err}");
                    return ExitCode::from(1);
                }
                break;
            }
            Err(err) => {
                println!("[tcp-server] Receive error: {err}");
                return ExitCode::from(1);
            }
        }
    }

    println!("[tcp-server] Done");
    ExitCode::SUCCESS
}
