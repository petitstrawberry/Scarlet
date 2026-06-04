use std::env;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::process::ExitCode;

fn main() -> ExitCode {
    println!("[tcp-client] Rust std version");

    let args = env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        println!("Usage: tcp_client <host> <port>");
        println!("Example: tcp_client 10.0.2.2 8080");
        return ExitCode::from(1);
    }

    let host = match args[1].parse::<Ipv4Addr>() {
        Ok(host) => host,
        Err(err) => {
            println!("[tcp-client] Invalid IPv4 address: {err}");
            return ExitCode::from(1);
        }
    };
    let port = match args[2].parse::<u16>() {
        Ok(port) if port != 0 => port,
        _ => {
            println!("[tcp-client] Invalid port number");
            return ExitCode::from(1);
        }
    };

    let addr = SocketAddrV4::new(host, port);
    println!("[tcp-client] Connecting to {addr}...");

    let mut stream = match TcpStream::connect(addr) {
        Ok(stream) => stream,
        Err(err) => {
            println!("[tcp-client] Connection failed: {err}");
            return ExitCode::from(1);
        }
    };

    println!("[tcp-client] Connected");

    let message = b"Hello from Scarlet TCP client!";
    match stream.write(message) {
        Ok(n) => println!("[tcp-client] Sent {n} bytes"),
        Err(err) => {
            println!("[tcp-client] Send failed: {err}");
            return ExitCode::from(1);
        }
    }

    let mut buf = [0; 1024];
    match stream.read(&mut buf) {
        Ok(0) => println!("[tcp-client] Server closed connection"),
        Ok(n) => println!("[tcp-client] Received {n} bytes"),
        Err(err) => {
            println!("[tcp-client] Receive failed: {err}");
            return ExitCode::from(1);
        }
    }

    println!("[tcp-client] Done");
    ExitCode::SUCCESS
}
