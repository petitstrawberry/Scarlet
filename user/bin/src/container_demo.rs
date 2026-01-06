//! Container Demo - Demonstrates Scarlet's namespace isolation features
//!
//! This program demonstrates how to create isolated execution environments
//! using Scarlet's smart CreateNamespace syscall.

#![no_std]
#![no_main]

extern crate scarlet_lib;

use scarlet_lib::{println, syscall::{Syscall, syscall0, syscall2}};

// Namespace creation flags (must match kernel definitions)
const NS_CREATE_TASK: usize = 0x01; // Create separate task namespace  
const NS_CREATE_VFS: usize = 0x02;  // Create separate VFS namespace

fn create_namespace(flags: usize, name: &str) -> Result<(), ()> {
    let name_ptr = name.as_ptr() as usize;
    let result = syscall2(Syscall::CreateNamespace, flags, name_ptr);
    
    if result == usize::MAX {
        Err(())
    } else {
        Ok(())
    }
}

fn getpid() -> usize {
    syscall0(Syscall::Getpid)
}

fn demo_task_namespace() {
    println!("\n=== Task Namespace Demo ===");
    println!("Parent process PID: {}", getpid());
    
    // Clone to create child
    let child_pid = syscall0(Syscall::Clone);
    
    if child_pid == 0 {
        // Child process
        println!("Child before namespace: PID = {}", getpid());
        
        // Create separate task namespace
        match create_namespace(NS_CREATE_TASK, "child_container") {
            Ok(()) => {
                println!("Child after namespace: PID = {} (in new namespace)", getpid());
                println!("Child: Successfully created isolated task namespace!");
            }
            Err(()) => {
                println!("Child: Failed to create task namespace");
            }
        }
        
        // Exit child
        syscall0(Syscall::Exit);
    } else {
        // Parent process
        println!("Parent created child with PID: {}", child_pid);
        
        // Wait for child
        let status_ptr: usize = 0; // null pointer
        syscall2(Syscall::Waitpid, child_pid, status_ptr);
        
        println!("Parent: Child completed");
    }
}

fn demo_combined_namespaces() {
    println!("\n=== Combined Namespace Demo (Task + VFS) ===");
    println!("Process PID before: {}", getpid());
    
    // Create both task and VFS namespaces
    match create_namespace(NS_CREATE_TASK | NS_CREATE_VFS, "full_container") {
        Ok(()) => {
            println!("Process PID after: {} (in new namespace)", getpid());
            println!("Successfully created isolated task AND VFS namespaces!");
            println!("This process now has:");
            println!("  - Independent PID space");
            println!("  - Isolated filesystem view");
            println!("  - Ready for containerized execution");
        }
        Err(()) => {
            println!("Failed to create combined namespaces");
        }
    }
}

fn demo_vfs_namespace() {
    println!("\n=== VFS Namespace Demo ===");
    println!("Current PID: {}", getpid());
    
    // Create VFS namespace only
    match create_namespace(NS_CREATE_VFS, "vfs_container") {
        Ok(()) => {
            println!("Successfully created isolated VFS namespace!");
            println!("This process now has an independent filesystem view");
            println!("Changes to mounts/filesystem won't affect other processes");
        }
        Err(()) => {
            println!("Failed to create VFS namespace");
        }
    }
}

#[no_mangle]
pub extern "C" fn main() -> i32 {
    println!("===========================================");
    println!("  Scarlet Container Demo");
    println!("  Demonstrating Namespace Isolation");
    println!("===========================================");
    
    // Demo 1: Task namespace isolation
    demo_task_namespace();
    
    // Demo 2: VFS namespace isolation
    demo_vfs_namespace();
    
    // Demo 3: Combined namespaces
    demo_combined_namespaces();
    
    println!("\n===========================================");
    println!("  Demo Complete!");
    println!("===========================================\n");
    
    0
}
