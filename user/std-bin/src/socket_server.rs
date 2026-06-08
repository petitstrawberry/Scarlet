use scarlet_os::socket::{ShutdownHow, Socket};
use std::process::ExitCode;

fn reverse_bytes(data: &mut [u8]) {
    let len = data.len();
    for i in 0..len / 2 {
        data.swap(i, len - 1 - i);
    }
}

fn main() -> ExitCode {
    println!("[socket-server] Rust std version");
    println!("========================================");
    println!("[Server] String Reverse Server v2.0");
    println!("[Server] Multiple client support enabled");
    println!("========================================");

    let server = match Socket::new() {
        Ok(socket) => socket,
        Err(err) => {
            println!("[Server] Failed to create socket: {err:?}");
            return ExitCode::from(1);
        }
    };

    let socket_path = "/tmp/reverse.sock";
    match server.bind(socket_path) {
        Ok(()) => println!("[Server] Listening on {socket_path}"),
        Err(err) => {
            println!("[Server] Failed to bind socket: {err:?}");
            return ExitCode::from(1);
        }
    }

    match server.listen(5) {
        Ok(()) => println!("[Server] Ready to accept connections"),
        Err(err) => {
            println!("[Server] Failed to listen: {err:?}");
            return ExitCode::from(1);
        }
    }

    loop {
        println!("[Server] Waiting for client connection...");

        let client = match server.accept() {
            Ok(client) => {
                println!("[Server] Client connected");
                client
            }
            Err(err) => {
                println!("[Server] Failed to accept connection: {err:?}");
                continue;
            }
        };

        let stream = match client.as_stream() {
            Ok(stream) => stream,
            Err(err) => {
                println!("[Server] Failed to get stream: {err:?}");
                continue;
            }
        };

        let mut buffer = [0u8; 256];

        loop {
            match stream.read(&mut buffer) {
                Ok(n) if n > 0 => {
                    let input = std::str::from_utf8(&buffer[..n]).unwrap_or("<invalid utf8>");
                    println!("[Server] Received: {input}");

                    reverse_bytes(&mut buffer[..n]);

                    let reversed = std::str::from_utf8(&buffer[..n]).unwrap_or("<invalid utf8>");
                    println!("[Server] Sending back: {reversed}");

                    if let Err(err) = stream.write_all(&buffer[..n]) {
                        println!("[Server] Failed to write: {err:?}");
                        break;
                    }
                }
                Ok(_) => {
                    println!("[Server] Client disconnected");
                    break;
                }
                Err(err) => {
                    println!("[Server] Read error: {err:?}");
                    break;
                }
            }
        }

        if let Err(err) = client.shutdown(ShutdownHow::Both) {
            println!("[Server] Shutdown error: {err:?}");
        }

        println!("[Server] Ready for next client");
    }
}
