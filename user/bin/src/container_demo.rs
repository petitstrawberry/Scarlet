//! Container Demo - Demonstrates Scarlet's namespace isolation features
//!
//! This program demonstrates how to create isolated execution environments
//! using Scarlet's smart CreateNamespace syscall.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    println,
    syscall::{Syscall, syscall2},
};

// Namespace creation flags (must match kernel definitions)
const NS_CREATE_TASK: usize = 0x01; // Create separate task namespace  
const NS_CREATE_VFS: usize = 0x02; // Create separate VFS namespace

// Syscall error return value
const SYSCALL_ERROR: usize = usize::MAX;

fn create_namespace(flags: usize, name: &str) -> Result<(), ()> {
    let name_cstr = std::ffi::str_to_cstr_bytes(name).map_err(|_| ())?;
    let name_ptr = name_cstr.as_ptr() as usize;
    let result = syscall2(Syscall::CreateNamespace, flags, name_ptr);

    if result == usize::MAX {
        Err(())
    } else {
        Ok(())
    }
}

fn getpid() -> usize {
    std::task::getpid() as usize
}

fn print_vfs_view(title: &str) {
    println!("--- VFS view: {} ---", title);

    match std::fs::get_cwd_path() {
        Ok(path) => println!("cwd: {}", path),
        Err(_) => println!("cwd: <unavailable>"),
    }

    match std::fs::list_directory("/") {
        Ok(entries) => {
            println!("/ entries: {}", entries.len());
            let mut shown = 0usize;
            for e in entries {
                if shown >= 32 {
                    println!("  ...");
                    break;
                }
                let kind = if e.is_directory() {
                    "dir"
                } else if e.is_file() {
                    "file"
                } else if e.is_symlink() {
                    "symlink"
                } else {
                    "other"
                };
                println!("  {}\t{}\t{} bytes", kind, e.name_str(), e.size);
                shown += 1;
            }
        }
        Err(_) => println!("/ entries: <unavailable>"),
    }
}

fn demo_task_namespace() {
    println!("\n=== Task Namespace Demo ===");
    println!("Parent process PID: {}", getpid());

    // Fork to create child
    let child_pid = std::task::fork();

    if child_pid == 0 {
        // Child process
        println!("Child before namespace: PID = {}", getpid());

        // Create separate task namespace
        match create_namespace(NS_CREATE_TASK, "child_container") {
            Ok(()) => {
                println!(
                    "Child after namespace: PID = {} (in new namespace)",
                    getpid()
                );
                println!("Child: Successfully created isolated task namespace!");
            }
            Err(()) => {
                println!("Child: Failed to create task namespace");
            }
        }

        // Exit child
        std::task::exit(0);
    } else {
        // Parent process
        println!("Parent created child with PID: {}", child_pid);

        // Wait for child
        let _ = std::task::waitpid(child_pid, 0);

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

    print_vfs_view("before VFS namespace");

    // Create VFS namespace only
    match create_namespace(NS_CREATE_VFS, "vfs_container") {
        Ok(()) => {
            println!("Successfully created isolated VFS namespace!");
            println!("This process now has an independent filesystem view");
            println!("Changes to mounts/filesystem won't affect other processes");

            print_vfs_view("after VFS namespace");
        }
        Err(()) => {
            println!("Failed to create VFS namespace");
        }
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
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
