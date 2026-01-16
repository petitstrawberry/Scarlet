//! PTY (Pseudo Terminal) operations for Scarlet Terminal
//!
//! This module handles creating and managing pseudo terminals for running shells.

extern crate scarlet_std as std;

use std::println;

// POSIX PTY constants and syscalls
const TIOCSCTTY: u64 = 0x540E;
const O_RDWR: u32 = 2;
const O_NOCTTY: u32 = 0x100;

unsafe extern "C" {
    fn posix_openpt(flags: u32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname(fd: i32) -> *mut i8;
    fn ioctl(fd: i32, request: u64, ...) -> i32;
    fn fork() -> i32;
    fn execve(path: *const i8, argv: *const *const i8, envp: *const *const i8) -> i32;
    fn close(fd: i32) -> i32;
    fn dup2(old: i32, new: i32) -> i32;
    fn setsid() -> i32;
    fn open(path: *const i8, oflag: u32, ...) -> i32;
    fn _exit(code: i32) -> !;
    fn syscall(num: isize, ...) -> isize;
}

// Syscall numbers for RISC-V
const SYS_READ: isize = 63;
const SYS_WRITE: isize = 64;

/// PTY master-slave pair
pub struct PtyPair {
    pub master_fd: i32,
    pub slave_path: scarlet_std::string::String,
}

impl PtyPair {
    /// Create a new PTY pair using `posix_openpt()`
    pub fn create() -> Result<Self, &'static str> {
        println!("[pty] Creating PTY pair");

        unsafe {
            // Open PTY master
            let master_fd = posix_openpt(O_RDWR | O_NOCTTY);
            if master_fd < 0 {
                return Err("Failed to open PTY master");
            }

            // Grant access to slave PTY
            if grantpt(master_fd) < 0 {
                close(master_fd);
                return Err("grantpt failed");
            }

            // Unlock slave PTY
            if unlockpt(master_fd) < 0 {
                close(master_fd);
                return Err("unlockpt failed");
            }

            // Get slave PTY name
            let slave_name_ptr = ptsname(master_fd);
            if slave_name_ptr.is_null() {
                close(master_fd);
                return Err("ptsname failed");
            }

            // Convert to Rust string
            let mut slave_name_vec = scarlet_std::vec::Vec::new();
            let mut ptr = slave_name_ptr;
            loop {
                let c = *ptr as u8;
                if c == 0 {
                    break;
                }
                slave_name_vec.push(c as char);
                ptr = ptr.offset(1);
            }
            let slave_path: scarlet_std::string::String = slave_name_vec.into_iter().collect();

            println!("[pty] PTY created: master={}, slave={}", master_fd, slave_path);

            Ok(Self { master_fd, slave_path })
        }
    }

    /// Fork and execute a shell with the PTY slave as stdin/stdout/stderr
    pub fn spawn_shell(&self, shell_path: &str) -> Result<PtyShell, &'static str> {
        println!("[pty] Spawning shell: {}", shell_path);

        unsafe {
            let pid = fork();

            if pid < 0 {
                return Err("fork failed");
            }

            if pid == 0 {
                // Child process

                // Create new session
                if setsid() < 0 {
                    println!("[pty] setsid failed");
                    _exit(1);
                }

                // Open slave PTY
                // Convert Rust string to C string (null-terminated)
                let mut slave_path_bytes = scarlet_std::vec::Vec::new();
                for b in self.slave_path.bytes() {
                    slave_path_bytes.push(b);
                }
                slave_path_bytes.push(0); // null terminator
                let slave_fd = open(slave_path_bytes.as_ptr() as *const i8, O_RDWR);

                if slave_fd < 0 {
                    println!("[pty] Failed to open slave PTY");
                    _exit(1);
                }

                // Set controlling terminal
                ioctl(slave_fd, TIOCSCTTY, 0);

                // Duplicate slave PTY to stdin, stdout, stderr
                dup2(slave_fd, 0);
                dup2(slave_fd, 1);
                dup2(slave_fd, 2);

                // Close slave PTY fd (we have duplicates now)
                close(slave_fd);

                // Execute shell
                // Prepare C strings for execve
                let mut shell_path_bytes = scarlet_std::vec::Vec::new();
                for b in b"/bin/sh" {
                    shell_path_bytes.push(*b);
                }
                shell_path_bytes.push(0);

                let mut arg_i_bytes = scarlet_std::vec::Vec::new();
                for b in b"-i" {
                    arg_i_bytes.push(*b);
                }
                arg_i_bytes.push(0);

                let argv = [
                    shell_path_bytes.as_ptr() as *const i8,
                    arg_i_bytes.as_ptr() as *const i8,
                    core::ptr::null(),
                ];

                execve(
                    shell_path_bytes.as_ptr() as *const i8,
                    argv.as_ptr(),
                    core::ptr::null(),
                );

                // If execve returns, it failed
                println!("[pty] execve failed");
                _exit(1);
            }

            // Parent process
            println!("[pty] Shell spawned with PID: {}", pid);
            Ok(PtyShell { pid, master_fd: self.master_fd })
        }
    }
}

impl Drop for PtyPair {
    fn drop(&mut self) {
        println!("[pty] Closing PTY pair");
    }
}

/// Shell process running in PTY
pub struct PtyShell {
    pid: i32,
    master_fd: i32,
}

impl PtyShell {
    /// Check if shell is still running
    pub fn is_running(&self) -> bool {
        unsafe {
            // TODO: Use waitpid with WNOHANG
            true
        }
    }

    /// Write data to shell's stdin
    pub fn write(&mut self, data: &[u8]) -> Result<(), &'static str> {
        unsafe {
            let result = syscall(SYS_WRITE, self.master_fd as isize, data.as_ptr(), data.len());
            if result < 0 {
                return Err("Write to PTY failed");
            }
            Ok(())
        }
    }

    /// Read data from shell's stdout
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        unsafe {
            let result = syscall(SYS_READ, self.master_fd as isize, buf.as_mut_ptr(), buf.len());
            if result < 0 {
                return Err("Read from PTY failed");
            }
            Ok(result as usize)
        }
    }
}
