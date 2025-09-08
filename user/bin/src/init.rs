#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    format, fs::{create_directory, list_directory, mount, pivot_root, remove_directory, remove_file, File}, handle::Handle, println, task::{execve_with_flags, exit, fork, getpid, waitpid, EXECVE_FORCE_ABI_REBUILD}
};

// Global variables for standard I/O handles to hold references
static mut STDIN: Option<Handle> = None;
static mut STDOUT: Option<Handle> = None;
static mut STDERR: Option<Handle> = None;

fn setup_new_root() -> bool {
    println!("init: Setting up new root filesystem...");
    
    // 1. Mount ext2 filesystem from first available block device (e.g., /dev/vblk0)
    println!("init: Mounting ext2 for new root at /mnt/newroot");
    match mount("/dev/vblk0", "/mnt/newroot", "ext2", 0, Some("device=/dev/vblk0,rw")) {
        Ok(_) => {
            println!("init: ext2 root filesystem mounted successfully");
        }
        Err(_) => {
            println!("init: Failed to mount ext2 at /mnt/newroot, trying fallback...");
            // Fallback to tmpfs if ext2 fails
            println!("init: Falling back to tmpfs for new root");
            match mount("tmpfs", "/mnt/newroot", "tmpfs", 0, Some("size=50M")) {
                Ok(_) => {
                    println!("init: Fallback tmpfs mounted successfully");
                }
                Err(_) => {
                    println!("init: Failed to mount fallback tmpfs at /mnt/newroot");
                    return false;
                }
            }
        }
    }
    
    // 2. Create necessary directories in the new root
    println!("init: Creating necessary directories in new root");
    
    // 3. Copy essential binaries (update paths based on actual initramfs structure)
    // Copy from the actual location in initramfs
    // copy_dir("/bin", "/mnt/newroot/bin");
    copy_dir("/system/scarlet", "/mnt/newroot/system/scarlet");
    // copy_dir("/data", "/mnt/newroot/data");
    
    // Create old_root directory in the new root (where the old root will be moved)
    match create_directory("/mnt/newroot/old_root") {
        Ok(_) => {
            println!("init: Created old_root directory in new root");
        }
        Err(_) => {
            println!("init: Warning: Could not create old_root directory (may already exist)");
            // Continue anyway as it might already exist
        }
    }
    
    true
}

fn setup_devfs() -> Result<(), &'static str> {
    let _ = create_directory("/dev"); // Create /dev directory if it doesn't exist

    // Mount devfs at /dev
    if mount("devfs", "/dev", "devfs", 0, None).is_ok() {
        Ok(())
    } else {
        Err("Failed to mount devfs")
    }
}

fn check_block_devices() -> bool {
    println!("init: Checking for available block devices...");
    
    // List devices in /dev to see what's available
    match list_directory("/dev") {
        Ok(entries) => {
            println!("init: Available devices in /dev:");
            let mut block_device_found = false;
            for entry in entries {
                println!("init:   - {}", entry.name);
                // Check for common block device names
                if entry.name.starts_with("vblk") || entry.name.starts_with("vda") || entry.name.starts_with("sda") || entry.name.starts_with("hda") {
                    block_device_found = true;
                    println!("init:     ^ Block device detected!");
                }
            }
            block_device_found
        }
        Err(_) => {
            println!("init: Failed to list /dev directory");
            false
        }
    }
}

fn setup_stdio() {
    // Set up standard input, output, and error
    let tty_file = File::open("/dev/tty0").expect("Failed to open /dev/tty0");
    
    // Handle 0 - convert File to Handle
    let stdin_handle = tty_file.into_handle();
    // Handle 1 - duplicate stdin for stdout
    let stdout_handle = stdin_handle.duplicate().expect("Failed to duplicate stdin handle");
    // Handle 2 - duplicate stdin for stderr
    let stderr_handle = stdin_handle.duplicate().expect("Failed to duplicate stdin handle");

    // Store the handles in global variables
    unsafe {
        STDIN = Some(stdin_handle);
        STDOUT = Some(stdout_handle);
        STDERR = Some(stderr_handle);
    }

    println!("init: Standard I/O setup complete");
}

fn perform_pivot_root() -> bool {
    println!("init: Performing pivot_root operation...");
    
    // Pivot root: move current root to /mnt/newroot/old_root, make /mnt/newroot the new root
    match pivot_root("/mnt/newroot", "/mnt/newroot/old_root") {
        Ok(_) => {
            println!("init: pivot_root successful!");
            println!("init: New root is now active, old root accessible at /old_root");
            
            // Optional: Clean up the old root (in a real system, you might want to keep it for a while)
            // umount("/old_root", 0);
            
            true
        }
        Err(_) => {
            println!("init: pivot_root failed");
            false
        }
    }
}

// Copy a directory from src to dest recursively
fn copy_dir(src: &str, dest: &str) -> bool {
    println!("init: Copying directory from {} to {}", src, dest);
    
    // If destination directory exists, remove all its contents first, then remove the directory itself
    match list_directory(dest) {
        Ok(entries) => {
            println!("init: Destination directory {} exists, removing all contents first", dest);
            // Remove all entries in the destination directory
            for entry in entries {
                // Skip . and .. entries
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                
                let dest_entry_path = format!("{}/{}", dest, entry.name);
                if entry.is_directory() {
                    // Recursively remove subdirectory (this will handle nested contents)
                    copy_dir("/dev/null", &dest_entry_path); // Use dummy source to trigger cleanup
                    match remove_directory(&dest_entry_path) {
                        Ok(_) => (),
                        Err(_) => println!("init: Failed to remove directory: {}", dest_entry_path),
                    }
                } else {
                    match remove_file(&dest_entry_path) {
                        Ok(_) => (),
                        Err(_) => println!("init: Failed to remove file: {}", dest_entry_path),
                    }
                }
            }
            
            // Now remove the destination directory itself
            match remove_directory(dest) {
                Ok(_) => (),
                Err(_) => println!("init: Failed to remove destination directory: {}", dest),
            }
        }
        Err(_) => {
            // Directory doesn't exist, which is fine
            println!("init: Destination directory {} does not exist", dest);
        }
    }
    
    // Create destination directory
    match create_directory(dest) {
        Ok(_) => (),
        Err(_) => {
            println!("init: Failed to create directory: {}", dest);
            return false;
        }
    }
    
    // Use the new API to read directory entries
    match list_directory(src) {
        Ok(entries) => {
            println!("init: Successfully read directory entries from {}", src);
            for entry in entries {
                let src_path = format!("{}/{}", src, entry.name);
                let dest_path = format!("{}/{}", dest, entry.name);
                
                // Skip . and .. entries
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                
                if entry.is_directory() {
                    // Recursively copy subdirectory
                    copy_dir(&src_path, &dest_path);
                } else if entry.is_file() {
                    // Copy file
                    copy_file(&src_path, &dest_path);
                } else {
                    println!("init: Skipping special file: {}", src_path);
                }
            }
            true
        }
        Err(_) => {
            println!("init: Failed to read directory entries from {}", src);
            false
        }
    }
}

fn copy_file(src: &str, dest: &str) -> bool {
    // Read source file
    match File::open(src) {
        Ok(mut src_file) => {
            // Remove existing destination file if it exists (for overwrite support)
            let _ = remove_file(dest); // Ignore errors if file doesn't exist
            
            // Create destination file
            match File::create(dest) {
                Ok(mut dest_file) => {
                    println!("init: Copying file from {} to {}", src, dest);
                    let mut buffer = [0u8; 4096]; // Buffer size of 4KB
                    let mut total_bytes_copied = 0;
                    
                    loop {
                        match src_file.read(&mut buffer) {
                            Ok(0) => break, // EOF
                            Ok(bytes_read) => {
                                 
                                 // Write to destination file
                                match dest_file.write(&buffer[..bytes_read]) {
                                    Ok(bytes_written) if bytes_written == bytes_read => {
                                        total_bytes_copied += bytes_written;
                                        // Success, continue
                                    }
                                    Ok(bytes_written) => {
                                        println!("init: Partial write! Expected {}, wrote {} bytes to {}", 
                                                bytes_read, bytes_written, dest);
                                        return false;
                                    }
                                    Err(_) => {
                                        println!("init: Failed to write to destination file: {}", dest);
                                        return false;
                                    }
                                }
                            }
                            Err(_) => {
                                println!("init: Failed to read from source file: {}", src);
                                return false;
                            }
                        }
                    }
                    true
                }
                Err(e) => {
                    println!("init: Failed to create destination file: {}: {}", dest, e);
                    false
                }
            }
        }
        Err(_) => {
            println!("init: Failed to open source file: {}", src);
            false
        }
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    // Initialize the device filesystem
    if setup_devfs().is_err() {
        exit(-1); // Exit if we cannot set up the device filesystem
    }

    // Set up standard input, output, and error
    setup_stdio();

    println!("init: I'm the init process: PID={}", getpid());
    
    // Check for available block devices
    if check_block_devices() {
        println!("init: Block devices found, proceeding with ext2 mount");
    } else {
        println!("init: No block devices found, will fallback to tmpfs");
    }
    
    println!("init: Starting root filesystem transition...");
    
    // Demonstrate pivot_root functionality with ext2 support
    if setup_new_root() {
        if perform_pivot_root() {
            println!("init: Root filesystem transition completed successfully");
            
            // Mount devfs at /dev to make devices accessible
            println!("init: Setting up device filesystem...");
            match setup_devfs() {
                Ok(_) => println!("init: Device filesystem mounted at /dev"),
                Err(e) => {
                    println!("init: Failed to setup device filesystem: {}", e);
                    // Continue anyway, but devices might not be accessible
                }
            }
            
            // Verify the new root by trying to access files
            println!("init: Current working directory after pivot_root");
        } else {
            println!("init: Failed to pivot root, continuing with current root");
        }
    } else {
        println!("init: Failed to setup new root, continuing with current root");
    }

    std::profiler::dump_profiler_stats();
    
    println!("init: Starting login process...");

    match fork() {
        0 => {
            // Child process: Execute the login program
            // After pivot_root, try the most likely locations for login binary
            let login_paths = [
                "/system/scarlet/bin/login",
                "/scarlet/system/scarlet/bin/login", // In new root (copied from initramfs)
                "/old_root/system/scarlet/bin/login", // In old root (original initramfs)
            ];
            
            for login_path in &login_paths {
                println!("init: Trying to execute login at: {}", login_path);
                
                // Try to open the file first to see if it exists
                match File::open(login_path) {
                    Ok(_) => {
                        println!("init: Login binary exists at {}", login_path);
                    }
                    Err(_) => {
                        println!("init: Login binary not found at {}", login_path);
                        continue;
                    }
                }
                
                if execve_with_flags(login_path, &[login_path], &[], EXECVE_FORCE_ABI_REBUILD) == 0 {
                    // This should not be reached if execve succeeds
                    break;
                } else {
                    println!("init: Failed to execve {} (binary exists but execve failed)", login_path);
                }
            }
            
            println!("init: All login paths failed, exiting child process");
            exit(-1);
        }
        -1 => {
            println!("init: Failed to clone");
            loop {}
        }
        pid => {
            println!("init: Login process created, child PID: {}", pid);
            
            let res = loop {
                let res = waitpid(pid, 0);
                if res.0 < 0 {
                    // Any child process exits
                    continue;
                }
                break res; // Exit loop on success
            };

            println!("init: Child process (PID={}) exited with status: {}", res.0, res.1);
            if res.1 != 0 {
                println!("init: Child process exited with error");
            }
            println!("init: System shutdown - all processes terminated");
            loop {}
        }
    }
}