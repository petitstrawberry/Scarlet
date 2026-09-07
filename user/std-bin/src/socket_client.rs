use scarlet_os::socket::{ShutdownHow, Socket};
use std::io::{self, Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    println!("[socket-client] Rust std version");
    println!("=== Interactive String Reverse Client ===");
    println!("Type messages to reverse them. Type 'exit' to quit.");
    println!();

    let client = match Socket::new() {
        Ok(socket) => socket,
        Err(err) => {
            println!("Failed to create socket: {err:?}");
            return ExitCode::from(1);
        }
    };

    let socket_path = "/tmp/reverse.sock";
    println!("Connecting to {socket_path}...");

    match client.connect(socket_path) {
        Ok(()) => println!("Connected to server!"),
        Err(err) => {
            println!("Failed to connect: {err:?}");
            println!("Make sure the server is running first.");
            return ExitCode::from(1);
        }
    }

    println!();

    let stream = match client.as_stream() {
        Ok(stream) => stream,
        Err(err) => {
            println!("Failed to get stream: {err:?}");
            return ExitCode::from(1);
        }
    };

    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut input_buffer = [0u8; 256];

    loop {
        print!("> ");
        let _ = stdout.flush();

        let bytes_read = match stdin.read(&mut input_buffer) {
            Ok(n) => n,
            Err(err) => {
                println!();
                println!("Failed to read input: {err}");
                break;
            }
        };

        if bytes_read == 0 {
            println!();
            println!("End of input");
            break;
        }

        let mut input_len = bytes_read;
        if input_len > 0 && input_buffer[input_len - 1] == b'\n' {
            input_len -= 1;
        }
        if input_len > 0 && input_buffer[input_len - 1] == b'\r' {
            input_len -= 1;
        }

        let input = match std::str::from_utf8(&input_buffer[..input_len]) {
            Ok(input) => input,
            Err(_) => {
                println!("Invalid UTF-8 input");
                continue;
            }
        };

        if input.is_empty() {
            continue;
        }

        if input == "exit" {
            println!("Exiting...");
            break;
        }

        if let Err(err) = stream.write_all(input.as_bytes()) {
            println!("Failed to send: {err:?}");
            break;
        }

        let mut response_buffer = [0u8; 256];
        match stream.read(&mut response_buffer) {
            Ok(n) if n > 0 => {
                let received =
                    std::str::from_utf8(&response_buffer[..n]).unwrap_or("<invalid utf8>");
                println!("{received}");
            }
            Ok(_) => {
                println!("Server disconnected");
                break;
            }
            Err(err) => {
                println!("Failed to receive: {err:?}");
                break;
            }
        }
    }

    println!();
    println!("Closing connection...");
    match client.shutdown(ShutdownHow::Both) {
        Ok(()) => println!("Connection closed"),
        Err(err) => println!("Shutdown error: {err:?}"),
    }

    println!("Client finished");
    ExitCode::SUCCESS
}
