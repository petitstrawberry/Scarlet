//! Socket Handle Transfer Demo
//!
//! Demonstrates transferring kernel objects (SharedMemory) between tasks
//! through sockets, similar to Unix SCM_RIGHTS functionality.

#![no_std]
#![no_main]

extern crate scarlet_std;

use core::time::Duration;
use scarlet_std::ipc::{SharedMemory, permissions};
use scarlet_std::socket::{ShutdownHow, Socket};
use scarlet_std::task::{exit, fork, getpid, waitpid};
use scarlet_std::thread::sleep;
use scarlet_std::{print, println};

fn sleep_ms(ms: u64) {
    let _ = sleep(Duration::from_millis(ms));
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== Socket Handle Transfer Demo ===");
    println!("Demonstrating SharedMemory transfer between processes\n");

    // Shared memory layout constants
    const MESSAGE_OFFSET: usize = 0; // Parent's message starts at offset 0
    const MESSAGE_MAX_LEN: usize = 50; // Maximum message length
    const RESPONSE_OFFSET: usize = 60; // Child's response starts at offset 60
    const RESPONSE_MAX_LEN: usize = 40; // Maximum response length

    // Create a local socket for IPC
    let server_path = "/tmp/handle_transfer_demo.sock";

    let pid = fork();

    if pid == 0 {
        // Child process - client
        println!("[Child {}] Starting client...", getpid());

        // Create client socket
        let client_sock = match Socket::new() {
            Ok(sock) => {
                println!("[Child] Created client socket: {}", sock.as_raw());
                sock
            }
            Err(_) => {
                println!("[Child] Failed to create socket");
                exit(1);
            }
        };

        // Connect to server (retry briefly in case the parent isn't ready yet)
        let mut connected = false;
        for _ in 0..200 {
            if client_sock.connect(server_path).is_ok() {
                connected = true;
                break;
            }
            sleep_ms(5);
        }
        if !connected {
            println!("[Child] Failed to connect to server");
            exit(1);
        }
        println!("[Child] Connected to server");

        // Receive the shared memory handle (blocking syscall)
        match client_sock.recv_handle() {
            Ok(shmem_handle) => {
                let shmem = match SharedMemory::from_handle(shmem_handle) {
                    Ok(shmem) => shmem,
                    Err(_) => {
                        println!("[Child] Received non-shared-memory handle");
                        exit(1);
                    }
                };
                println!("[Child] Received shared memory handle: {}", shmem.as_raw());

                // Map the shared memory
                let mapper = match shmem.as_handle().as_memory_mapping() {
                    Ok(mapper) => mapper,
                    Err(_) => {
                        println!("[Child] SharedMemory does not support memory mapping");
                        let _ = client_sock.shutdown(ShutdownHow::Both);
                        println!("[Child] Exiting...");
                        exit(1);
                    }
                };
                match mapper.mmap(0, 4096, permissions::READ_WRITE, 0, 0) {
                    Ok(addr) => {
                        println!("[Child] Mapped shared memory at: {:#x}", addr);

                        // Read the message from shared memory
                        unsafe {
                            let ptr = addr as *const u8;
                            print!("[Child] Message from parent: \"");
                            for i in MESSAGE_OFFSET..MESSAGE_MAX_LEN {
                                let c = *ptr.add(i) as char;
                                if c == '\0' {
                                    break;
                                }
                                print!("{}", c);
                            }
                            println!("\"");

                            // Write a response
                            let response = b"Hello from child!";
                            let ptr_mut = addr as *mut u8;
                            for (i, &byte) in response.iter().enumerate() {
                                *ptr_mut.add(RESPONSE_OFFSET + i) = byte;
                            }
                            *ptr_mut.add(RESPONSE_OFFSET + response.len()) = 0; // Null terminator
                        }

                        println!("[Child] Wrote response to shared memory");
                    }
                    Err(e) => {
                        println!("[Child] Failed to map shared memory: {:?}", e);
                    }
                }
            }
            Err(_) => {
                println!("[Child] Failed to receive handle");
                let _ = client_sock.shutdown(ShutdownHow::Both);
                println!("[Child] Exiting...");
                exit(1);
            }
        }

        // Cleanup
        let _ = client_sock.shutdown(ShutdownHow::Both);
        println!("[Child] Exiting...");
        exit(0);
    } else if pid > 0 {
        // Parent process - server
        println!("[Parent {}] Starting server...", getpid());
        println!("[Parent] Child PID: {}", pid);

        // Create server socket
        let server_sock = match Socket::new() {
            Ok(sock) => {
                println!("[Parent] Created server socket: {}", sock.as_raw());
                sock
            }
            Err(_) => {
                println!("[Parent] Failed to create socket");
                exit(1);
            }
        };

        // Bind to path
        if server_sock.bind(server_path).is_err() {
            println!("[Parent] Failed to bind socket");
            exit(1);
        }
        println!("[Parent] Bound to {}", server_path);

        // Listen for connections
        if server_sock.listen(1).is_err() {
            println!("[Parent] Failed to listen");
            exit(1);
        }
        println!("[Parent] Listening for connections...");

        // Accept connection (blocking in kernel)
        let client_conn = match server_sock.accept() {
            Ok(sock) => sock,
            Err(_) => {
                println!("[Parent] Failed to accept connection");
                exit(1);
            }
        };
        println!("[Parent] Accepted connection: {}", client_conn.as_raw());

        // Create shared memory
        let shmem = match SharedMemory::create(4096, permissions::READ_WRITE) {
            Ok(shm) => {
                println!("[Parent] Created shared memory: {}", shm.as_raw());
                shm
            }
            Err(_) => {
                println!("[Parent] Failed to create shared memory");
                exit(1);
            }
        };

        // Map shared memory and write a message
        let mapper = match shmem.as_handle().as_memory_mapping() {
            Ok(mapper) => mapper,
            Err(_) => {
                println!("[Parent] SharedMemory does not support memory mapping");
                exit(1);
            }
        };
        match mapper.mmap(0, 4096, permissions::READ_WRITE, 0, 0) {
            Ok(addr) => {
                println!("[Parent] Mapped shared memory at: {:#x}", addr);

                // Write a message
                let message = b"Hello from parent via shared memory!";
                unsafe {
                    let ptr = addr as *mut u8;
                    for (i, &byte) in message.iter().enumerate() {
                        *ptr.add(i) = byte;
                    }
                    *ptr.add(message.len()) = 0; // Null terminator
                }
                println!("[Parent] Wrote message to shared memory");
            }
            Err(e) => {
                println!("[Parent] Failed to map shared memory: {:?}", e);
            }
        }

        // Send the shared memory handle through the socket
        let sent_ok = client_conn.send_handle(shmem.as_handle()).is_ok();
        if sent_ok {
            println!("[Parent] Successfully sent shared memory handle!");
        } else {
            println!("[Parent] Failed to send handle");
        }

        // Wait for child to exit (child writes the response before exiting)
        println!("[Parent] Waiting for child to exit...");
        let _ = waitpid(pid, 0);
        println!("[Parent] Child exited");

        // Read the response from shared memory
        let mut response_non_empty = false;
        let mapper = match shmem.as_handle().as_memory_mapping() {
            Ok(mapper) => mapper,
            Err(_) => {
                println!("[Parent] SharedMemory does not support memory mapping");
                println!("[Parent] Response from child: \"\"");
                let _ = client_conn.shutdown(ShutdownHow::Both);
                let _ = server_sock.shutdown(ShutdownHow::Both);
                println!("\nHandle transfer test failed");
                return 1;
            }
        };
        if let Ok(addr) = mapper.mmap(0, 4096, permissions::READ_WRITE, 0, 0) {
            unsafe {
                let ptr = addr as *const u8;
                response_non_empty = *ptr.add(RESPONSE_OFFSET) != 0;
                print!("[Parent] Response from child: \"");
                for i in RESPONSE_OFFSET..(RESPONSE_OFFSET + RESPONSE_MAX_LEN) {
                    let c = *ptr.add(i) as char;
                    if c == '\0' {
                        break;
                    }
                    print!("{}", c);
                }
                println!("\"");
            }
        } else {
            println!("[Parent] Response from child: \"\"");
        }

        // Cleanup
        let _ = client_conn.shutdown(ShutdownHow::Both);
        let _ = server_sock.shutdown(ShutdownHow::Both);

        if sent_ok && response_non_empty {
            println!("\n✓ Handle transfer test completed successfully!");
            return 0;
        } else {
            println!("\nHandle transfer test failed");
            return 1;
        }
    } else {
        println!("Fork failed!");
        return 1;
    }
}
