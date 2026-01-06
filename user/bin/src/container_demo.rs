//! Container Demo - Demonstrates Scarlet's namespace isolation features
//!
//! This program demonstrates how to create isolated execution environments
//! using Scarlet's smart CreateNamespace syscall.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    fs::OpenOptions,
    println,
    syscall::{Syscall, syscall2},
};

// Namespace creation flags (must match kernel definitions)
const NS_CREATE_TASK: usize = 0x01; // Create separate task namespace  
const NS_CREATE_VFS: usize = 0x02; // Create separate VFS namespace

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

fn print_dir(path: &str, title: &str) {
    println!("--- dir: {} ({}) ---", path, title);
    match std::fs::list_directory(path) {
        Ok(entries) => {
            println!("{} entries: {}", path, entries.len());
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
        Err(_) => println!("{} entries: <unavailable>", path),
    }
}

fn dir_contains_entry(path: &str, entry_name: &str) -> Option<bool> {
    match std::fs::list_directory(path) {
        Ok(entries) => Some(entries.iter().any(|e| e.name_str() == entry_name)),
        Err(_) => None,
    }
}

fn setup_tmpfs_newroot() -> Result<(), ()> {
    let _ = std::fs::create_directory("/newroot");
    std::fs::mount("tmpfs", "/newroot", "tmpfs", 0, Some("size=50M")).map_err(|_| ())?;
    let _ = std::fs::create_directory("/newroot/old_root");
    Ok(())
}

fn pivot_into_newroot() -> Result<(), ()> {
    std::fs::pivot_root("/newroot", "/newroot/old_root").map_err(|_| ())?;
    let _ = std::fs::change_directory("/");
    Ok(())
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

    // Isolation verification: create a file inside a VFS namespace and ensure
    // it is NOT visible from this (parent) process.
    println!("\n--- VFS isolation check: file visibility ---");
    // NOTE: Don't touch parent FS (no create/remove on parent root).
    let test_file = "/__scarlet_vfs_ns_isolation_test";
    let test_file_name = "__scarlet_vfs_ns_isolation_test";

    let child_pid = std::task::fork();
    if child_pid == 0 {
        // Child process: enter a new VFS namespace, pivot_root to tmpfs, then create a file.
        match create_namespace(NS_CREATE_VFS, "vfs_isolation_child") {
            Ok(()) => {
                if setup_tmpfs_newroot().is_err() {
                    println!("Child: mount tmpfs failed");
                    std::task::exit(2);
                }

                if pivot_into_newroot().is_err() {
                    println!("Child: pivot_root failed");
                    std::task::exit(3);
                }

                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(test_file)
                {
                    Ok(mut file) => {
                        let _ = file.write_all(b"hello from vfs namespace\n");
                        println!("Child: created {} inside VFS namespace", test_file);
                        std::task::exit(0);
                    }
                    Err(e) => {
                        println!("Child: create {} failed: {}", test_file, e);
                        std::task::exit(4);
                    }
                }
            }
            Err(()) => {
                println!("Child: failed to create VFS namespace");
                std::task::exit(1);
            }
        }
    } else {
        // Parent process: wait for the child, then ensure the file is not visible here.
        let (_waited_pid, status) = std::task::waitpid(child_pid, 0);
        if status != 0 {
            println!("Parent: ERROR - child test failed (status={})", status);
        } else {
            match dir_contains_entry("/", test_file_name) {
                Some(true) => println!(
                    "Parent: ERROR - {} is visible (isolation broken)",
                    test_file
                ),
                Some(false) => println!(
                    "Parent: OK - {} is not visible (isolation verified)",
                    test_file
                ),
                None => println!("Parent: WARN - cannot list '/', skipping visibility assertion"),
            }
        }
    }

    // Create VFS namespace only
    match create_namespace(NS_CREATE_VFS, "vfs_container") {
        Ok(()) => {
            println!("Successfully created isolated VFS namespace!");
            println!("This process now has an independent filesystem view");
            println!("Changes to mounts/filesystem won't affect other processes");

            print_vfs_view("after VFS namespace");

            // Demonstrate pivot_root inside the isolated VFS namespace.
            // Minimal flow: mount tmpfs at /newroot, create /newroot/old_root, then pivot_root.
            println!("\n--- pivot_root demo (in VFS namespace) ---");

            match setup_tmpfs_newroot() {
                Ok(()) => println!("mount tmpfs: OK"),
                Err(()) => {
                    println!("mount tmpfs: ERR");
                    return;
                }
            }

            // Inspect what newroot looks like before pivot.
            print_dir("/newroot", "before pivot_root");

            match pivot_into_newroot() {
                Ok(()) => println!(
                    "pivot_root successful: new_root='/newroot' old_root='/newroot/old_root'"
                ),
                Err(()) => println!("pivot_root failed"),
            }

            print_vfs_view("after pivot_root");
            // Inspect old_root after pivot (init expects it at /old_root).
            print_dir("/old_root", "after pivot_root");
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
