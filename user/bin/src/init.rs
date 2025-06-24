#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{format, fs::{self, close, mkdir, mkfile, mount, open, pivot_root, readdir, umount}, println, task::{execve, exit, waitpid}, vec::Vec};

fn setup_new_root() -> bool {
    println!("init: Setting up new root filesystem...");
    
    // 1. Create a tmpfs for demonstration (in a real system, this might be mounting a real device)
    println!("init: Creating tmpfs for new root at /mnt/newroot");
    if mount("tmpfs", "/mnt/newroot", "tmpfs", 0, Some("size=50M")) != 0 {
        println!("init: Failed to mount tmpfs at /mnt/newroot");
        return false;
    }
    
    // 2. Create necessary directories in the new root
    // Note: In a real implementation, we'd need mkdir syscall or use existing directories
    println!("init: New root filesystem mounted successfully");
    
    // 3. Copy essential binaries (in practice, these would already exist in the new filesystem)
    // For this demo, we'll assume /mnt/newroot already has the necessary structure
    copy_dir("/bin", "/mnt/newroot/bin");
    copy_dir("/system", "/mnt/newroot/system");
    copy_dir("/data", "/mnt/newroot/data");
    // mkdir("/mnt/newroot/bin", 0); // Create /bin directory in new root
    // copy_file("/bin/sh", "/mnt/newroot/bin/sh"); // Copy shell binary
    // copy_file("/bin/hello", "/mnt/newroot/bin/hello"); // Copy hello binary

    // mkdir("/mnt/newroot/bin", 0);
    // mount("/bin", "/mnt/newroot/bin", "bind", 0, None);

    // mkdir("/mnt/newroot/system/", 0);
    // mount("/system", "/mnt/newroot/system", "bind", 0, None);
    
    // 4. Create old_root directory in the new root (where the old root will be moved)
    // Again, this would typically require mkdir, but we'll assume it exists
    
    true
}

fn perform_pivot_root() -> bool {
    println!("init: Performing pivot_root operation...");
    
    // Pivot root: move current root to /mnt/newroot/old_root, make /mnt/newroot the new root
    if pivot_root("/mnt/newroot", "/mnt/newroot/old_root") != 0 {
        println!("init: pivot_root failed");
        return false;
    }
    
    println!("init: pivot_root successful!");
    println!("init: New root is now active, old root accessible at /old_root");
    
    // Optional: Clean up the old root (in a real system, you might want to keep it for a while)
    umount("/old_root", 0);
    
    true
}

// Copy a directory from src to dest recursively
fn copy_dir(src: &str, dest: &str) -> bool {
    let src_dir = open(src, 0);
    if src_dir < 0 {
        println!("init: Failed to open source directory: {}", src);
        return false;
    }

    let dest_dir = open(dest, 0); // Open for writing
    if dest_dir < 0 { // If the destination directory does not exist, we should create it
        if std::fs::mkdir(dest, 0) < 0 {
            println!("init: Failed to create destination directory: {}", dest);
            close(src_dir);
            return false;
        }
    }

    loop {
        let entry = match readdir(src_dir) {
            Ok(Some(entry)) => entry,
            Ok(None) => break, // No more entries
            Err(e) => {
                println!("init: Failed to read directory {}: {}", src, e);
                close(src_dir);
                close(dest_dir);
                return false;
            }
        };
        let src_path = format!("{}/{}", src, entry.name);
        let dest_path = format!("{}/{}", dest, entry.name);

        // Skip the current directory (.) and parent directory (..)
        if entry.name == "." || entry.name == ".." {
            continue;
        }

        if entry.is_file() {
            copy_file(&src_path, &dest_path);
        } else if entry.is_directory() {
            // Recursively copy the directory
            if !copy_dir(&src_path, &dest_path) {
                println!("init: Failed to copy directory {} to {}", src_path, dest_path);
                close(src_dir);
                close(dest_dir);
                return false;
            }
        }
    }

    close(src_dir);
    close(dest_dir);

    true
}

fn copy_file(src: &str, dest: &str) -> bool {
    mkfile(dest, 0); // Create the destination file if it doesn't exist
    let src_fd = open(src, 0);
    if src_fd < 0 {
        println!("init: Failed to open source file: {}", src);
        return false;
    }
    
    let dest_fd = open(dest, 0); // Open for writing
    if dest_fd < 0 {
        println!("init: Failed to open destination file: {}", dest);
        close(src_fd);
        return false;
    }
    
    println!("init: Copying file from {} to {}", src, dest);
    let mut buffer = [0u8; 4096]; // Buffer size of 4KB
    loop {
        let bytes_read = std::fs::read(src_fd, &mut buffer);
        if bytes_read <= 0 {
            break; // EOF or error
        }
        let bytes_read = bytes_read as usize;
        if std::fs::write(dest_fd, &buffer[..bytes_read]) != bytes_read as i32 {
            println!("init: Failed to write to destination file: {}", dest);
            close(src_fd);
            close(dest_fd);
            return false;
        }
    }

    close(src_fd);
    close(dest_fd);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("init: I'm the init process: PID={}", std::task::getpid());
    println!("init: Starting root filesystem transition...");
    
    // Demonstrate pivot_root functionality
    if setup_new_root() {
        if perform_pivot_root() {
            println!("init: Root filesystem transition completed successfully");
            
            // Verify the new root by trying to access files
            println!("init: Current working directory after pivot_root");
            // In a real system, you'd verify that essential files are accessible
            
        } else {
            println!("init: Failed to pivot root, continuing with current root");
        }
    } else {
        println!("init: Failed to setup new root, continuing with current root");
    }
    
    println!("init: Starting shell process...");

    match std::task::fork() {
        0 => {
            // Child process: Execute the shell program
            if execve("/bin/sh", &[], &[]) != 0 {
                println!("Failed to execve /bin/sh");
                // Try to execute from old root if pivot_root was successful
                if execve("/old_root/bin/sh", &[], &[]) != 0 {
                    println!("Failed to execve /old_root/bin/sh");
                }
            }
            exit(-1);
        }
        -1 => {
            println!("init: Failed to clone");
            loop {}
        }
        pid => {
            println!("init: Shell process created, child PID: {}", pid);
            let res = waitpid(pid, 0);
            println!("init: Child process (PID={}) exited with status: {}", res.0, res.1);
            if res.1 != 0 {
                println!("init: Child process exited with error");
            }
            println!("init: System shutdown - all processes terminated");
            loop {}
        }
    }
}