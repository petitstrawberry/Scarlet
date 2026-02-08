//! Shared Memory Example
//!
//! Demonstrates the usage of shared memory IPC in Scarlet OS.

#![no_std]
#![no_main]

extern crate scarlet_std;

use scarlet_std::arch::*;
use scarlet_std::ipc::{permissions, SharedMemory};
use scarlet_std::println;

#[no_mangle]
pub fn _start() -> ! {
    println!("=== Shared Memory Example ===\n");

    // Create a 4KB shared memory region
    let size = 4096;
    println!("Creating shared memory region of {} bytes...", size);

    match SharedMemory::create(size, permissions::READ_WRITE) {
        Ok(shm) => {
            println!("✓ Shared memory created successfully!");
            println!("  Handle: {}", shm.as_raw());

            // Map the shared memory into our address space
            println!("\nMapping shared memory into address space...");
            let mapper = shm.as_handle().as_memory_mapping().unwrap();
            match mapper.mmap(0, size, permissions::READ_WRITE, 0, 0) {
                Ok(addr) => {
                    println!("✓ Memory mapped at address: {:#x}", addr);

                    // Write some data to the shared memory
                    println!("\nWriting data to shared memory...");
                    unsafe {
                        let ptr = addr as *mut u8;
                        let message = b"Hello from shared memory!";
                        for (i, &byte) in message.iter().enumerate() {
                            *ptr.add(i) = byte;
                        }
                        println!("✓ Wrote {} bytes", message.len());

                        // Read it back to verify
                        println!("\nReading data back:");
                        print!("  Content: \"");
                        for i in 0..message.len() {
                            let c = *ptr.add(i) as char;
                            print!("{}", c);
                        }
                        println!("\"");
                    }

                    println!("\n✓ Shared memory test completed successfully!");
                }
                Err(e) => {
                    println!("✗ Failed to map memory: {:?}", e);
                }
            }
        }
        Err(_) => {
            println!("✗ Failed to create shared memory");
        }
    }

    println!("\nExiting...");
    arch_exit(0);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("PANIC: {}", info);
    arch_exit(1);
}
