//! Socket Handle Transfer Demo
//!
//! Demonstrates transferring kernel objects (SharedMemory) between tasks
//! through sockets, similar to Unix SCM_RIGHTS functionality.

#![no_std]
#![no_main]

extern crate scarlet_std;

use core::time::Duration;
use scarlet_std::handle::capability::memory_mapping::mmap;
use scarlet_std::ipc::{permissions, shared_memory_create, socket_recv_handle, socket_send_handle};
use scarlet_std::socket::{ShutdownHow, Socket};
use scarlet_std::syscall::{Syscall, syscall1};
use scarlet_std::task::{exit, fork, getpid, waitpid};
use scarlet_std::thread::sleep;
use scarlet_std::{print, println};

/// Close a handle
fn close_handle(handle: u32) {
    let _ = syscall1(Syscall::HandleClose, handle as usize);
}

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
                println!("[Child] Created client socket: {}", sock.as_raw_handle());
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
            close_handle(client_sock.as_raw_handle());
            exit(1);
        }
        println!("[Child] Connected to server");

        // Receive the shared memory handle (blocking syscall)
        match socket_recv_handle(client_sock.as_raw_handle()) {
            Some(shmem_handle) => {
                println!("[Child] Received shared memory handle: {}", shmem_handle);

                // Map the shared memory
                match mmap(shmem_handle, 0, 4096, permissions::READ_WRITE, 0, 0) {
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

                close_handle(shmem_handle);
            }
            None => {
                println!("[Child] Failed to receive handle");
                let _ = client_sock.shutdown(ShutdownHow::Both);
                close_handle(client_sock.as_raw_handle());
                println!("[Child] Exiting...");
                exit(1);
            }
        }

        // Cleanup
        let _ = client_sock.shutdown(ShutdownHow::Both);
        close_handle(client_sock.as_raw_handle());
        println!("[Child] Exiting...");
        exit(0);
    } else if pid > 0 {
        // Parent process - server
        println!("[Parent {}] Starting server...", getpid());
        println!("[Parent] Child PID: {}", pid);

        // Create server socket
        let server_sock = match Socket::new() {
            Ok(sock) => {
                println!("[Parent] Created server socket: {}", sock.as_raw_handle());
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
            close_handle(server_sock.as_raw_handle());
            exit(1);
        }
        println!("[Parent] Bound to {}", server_path);

        // Listen for connections
        if server_sock.listen(1).is_err() {
            println!("[Parent] Failed to listen");
            close_handle(server_sock.as_raw_handle());
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
        println!(
            "[Parent] Accepted connection: {}",
            client_conn.as_raw_handle()
        );

        // Create shared memory
        let shmem_handle = match shared_memory_create(4096, permissions::READ_WRITE) {
            Some(handle) => {
                println!("[Parent] Created shared memory: {}", handle);
                handle
            }
            None => {
                println!("[Parent] Failed to create shared memory");
                close_handle(client_conn.as_raw_handle());
                close_handle(server_sock.as_raw_handle());
                exit(1);
            }
        };

        // Map shared memory and write a message
        match mmap(shmem_handle, 0, 4096, permissions::READ_WRITE, 0, 0) {
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
        let sent_ok = socket_send_handle(client_conn.as_raw_handle(), shmem_handle);
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
        if let Ok(addr) = mmap(shmem_handle, 0, 4096, permissions::READ_WRITE, 0, 0) {
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
        close_handle(shmem_handle);
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
