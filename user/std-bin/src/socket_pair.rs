use scarlet_os::socket::{ShutdownHow, Socket};
use std::process::ExitCode;

fn main() -> ExitCode {
    println!("[socket-pair] Rust std version");
    println!("=== Scarlet Socket Pair Example ===");

    let (socket1, socket2) = match Socket::pair() {
        Ok(pair) => pair,
        Err(err) => {
            println!("Failed to create socket pair: {err:?}");
            return ExitCode::from(1);
        }
    };

    println!("Created connected socket pair");
    println!("Socket 1 handle: {}", socket1.as_raw());
    println!("Socket 2 handle: {}", socket2.as_raw());

    let stream1 = match socket1.as_stream() {
        Ok(stream) => stream,
        Err(err) => {
            println!("Failed to get stream from socket 1: {err:?}");
            return ExitCode::from(1);
        }
    };

    let stream2 = match socket2.as_stream() {
        Ok(stream) => stream,
        Err(err) => {
            println!("Failed to get stream from socket 2: {err:?}");
            return ExitCode::from(1);
        }
    };

    println!();
    println!("--- Test 1: Socket 1 -> Socket 2 ---");
    let msg1 = b"Hello from Socket 1!";
    println!("Socket 1 sending: {:?}", std::str::from_utf8(msg1).unwrap());

    match stream1.write_all(msg1) {
        Ok(()) => println!("Socket 1 sent {} bytes", msg1.len()),
        Err(err) => {
            println!("Failed to send from socket 1: {err:?}");
            return ExitCode::from(1);
        }
    }

    let mut buffer1 = [0u8; 256];
    match stream2.read(&mut buffer1) {
        Ok(n) if n > 0 => {
            let received = std::str::from_utf8(&buffer1[..n]).unwrap_or("<invalid utf8>");
            println!("Socket 2 received: {received}");
        }
        Ok(_) => println!("Socket 2 received no data"),
        Err(err) => {
            println!("Failed to read from socket 2: {err:?}");
            return ExitCode::from(1);
        }
    }

    println!();
    println!("--- Test 2: Socket 2 -> Socket 1 ---");
    let msg2 = b"Hello from Socket 2!";
    println!("Socket 2 sending: {:?}", std::str::from_utf8(msg2).unwrap());

    match stream2.write_all(msg2) {
        Ok(()) => println!("Socket 2 sent {} bytes", msg2.len()),
        Err(err) => {
            println!("Failed to send from socket 2: {err:?}");
            return ExitCode::from(1);
        }
    }

    let mut buffer2 = [0u8; 256];
    match stream1.read(&mut buffer2) {
        Ok(n) if n > 0 => {
            let received = std::str::from_utf8(&buffer2[..n]).unwrap_or("<invalid utf8>");
            println!("Socket 1 received: {received}");
        }
        Ok(_) => println!("Socket 1 received no data"),
        Err(err) => {
            println!("Failed to read from socket 1: {err:?}");
            return ExitCode::from(1);
        }
    }

    println!();
    println!("--- Test 3: Bidirectional Exchange ---");
    let msg3 = b"Ping";
    println!("Socket 1 sending: {:?}", std::str::from_utf8(msg3).unwrap());

    if let Err(err) = stream1.write_all(msg3) {
        println!("Failed to send ping: {err:?}");
        return ExitCode::from(1);
    }

    let mut buffer3 = [0u8; 256];
    match stream2.read(&mut buffer3) {
        Ok(n) if n > 0 => {
            println!(
                "Socket 2 received: {:?}",
                std::str::from_utf8(&buffer3[..n]).unwrap_or("<invalid utf8>")
            );
        }
        Ok(_) => println!("Socket 2 received no data"),
        Err(err) => {
            println!("Failed to read ping: {err:?}");
            return ExitCode::from(1);
        }
    }

    let response = b"Pong";
    println!(
        "Socket 2 responding: {:?}",
        std::str::from_utf8(response).unwrap()
    );
    if let Err(err) = stream2.write_all(response) {
        println!("Failed to send pong: {err:?}");
        return ExitCode::from(1);
    }

    let mut buffer4 = [0u8; 256];
    match stream1.read(&mut buffer4) {
        Ok(n) if n > 0 => {
            println!(
                "Socket 1 received: {:?}",
                std::str::from_utf8(&buffer4[..n]).unwrap_or("<invalid utf8>")
            );
        }
        Ok(_) => println!("Socket 1 received no data"),
        Err(err) => {
            println!("Failed to read pong: {err:?}");
            return ExitCode::from(1);
        }
    }

    println!();
    println!("--- Cleanup ---");
    let _ = socket1.shutdown(ShutdownHow::Both);
    let _ = socket2.shutdown(ShutdownHow::Both);
    println!("Both sockets closed");

    println!();
    println!("Socket pair example finished successfully");
    ExitCode::SUCCESS
}
