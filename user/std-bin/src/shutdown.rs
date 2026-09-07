use scarlet_os::socket::Socket;
use std::process::ExitCode;

fn main() -> ExitCode {
    // Shutdown flow:
    // 1. shutdown command (this) sends request to stemd
    // 2. stemd performs proper cleanup (signals, sync, unmount)
    // 3. stemd calls kernel shutdown syscall as final step
    // 4. kernel force-kills any remaining tasks and powers off
    println!("[shutdown] Requesting shutdown from stemd...");

    let socket_path = "/tmp/stemd.sock";

    let socket = match Socket::new() {
        Ok(socket) => socket,
        Err(error) => {
            println!("[shutdown] Failed to create socket: {error:?}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = socket.connect(socket_path) {
        println!("[shutdown] Failed to connect to stemd: {error:?}");
        return ExitCode::FAILURE;
    }

    let stream = match socket.as_stream() {
        Ok(stream) => stream,
        Err(error) => {
            println!("[shutdown] Failed to get stream: {error:?}");
            return ExitCode::FAILURE;
        }
    };

    let command = [4u8];
    if let Err(error) = stream.write(&command) {
        println!("[shutdown] Failed to send command: {error:?}");
        return ExitCode::FAILURE;
    }

    let mut buffer = [0u8; 256];
    match stream.read(&mut buffer) {
        Ok(bytes_read) if bytes_read > 0 => {
            if let Ok(response) = std::str::from_utf8(&buffer[..bytes_read]) {
                println!("[shutdown] Response: {}", response.trim());
            }
        }
        _ => {
            println!("[shutdown] No response from stemd");
        }
    }

    ExitCode::SUCCESS
}
