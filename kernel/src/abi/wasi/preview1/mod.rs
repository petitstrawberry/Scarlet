//! WASI Preview 1 Implementation
//!
//! This module implements WASI Preview 1 system calls as defined by the
//! WebAssembly System Interface specification.

#[macro_use]
mod macros;

use alloc::{boxed::Box, string::{String, ToString}, sync::Arc, vec::Vec};
use hashbrown::HashMap;

use crate::{
    abi::AbiModule,
    arch::{IntRegisters, Trapframe},
    early_initcall,
    fs::{FileSystemError, FileSystemErrorKind, SeekFrom, VfsManager},
    register_abi,
    task::elf_loader::load_elf_into_task,
};

const MAX_FDS: usize = 1024; // Maximum number of file descriptors

/// WASI Preview 1 ABI implementation
#[derive(Clone)]
pub struct WasiPreview1Abi {
    /// File descriptor to handle mapping (fd -> handle)
    fd_to_handle: HashMap<usize, u32>,
    /// Free file descriptor list for O(1) allocation/deallocation
    free_fds: Vec<usize>,
}

impl Default for WasiPreview1Abi {
    fn default() -> Self {
        // Initialize free_fds with all available file descriptors (0 to MAX_FDS-1)
        // Pop from the end so fd 0, 1, 2 are allocated first
        let mut free_fds: Vec<usize> = (0..MAX_FDS).collect();
        free_fds.reverse(); // Reverse so fd 0 is at the end and allocated first
        Self {
            fd_to_handle: HashMap::new(),
            free_fds,
        }
    }
}

impl WasiPreview1Abi {
    /// Allocate a new file descriptor and map it to a handle
    pub fn allocate_fd(&mut self, handle: u32) -> Result<usize, &'static str> {
        let fd = if let Some(freed_fd) = self.free_fds.pop() {
            freed_fd
        } else {
            return Err("Too many open files");
        };
        
        self.fd_to_handle.insert(fd, handle);
        Ok(fd)
    }
    
    /// Get handle from file descriptor
    pub fn get_handle(&self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            self.fd_to_handle.get(&fd).copied()
        } else {
            None
        }
    }
    
    /// Remove file descriptor mapping
    pub fn remove_fd(&mut self, fd: usize) -> Option<u32> {
        if fd < MAX_FDS {
            if let Some(handle) = self.fd_to_handle.remove(&fd) {
                self.free_fds.push(fd);
                Some(handle)
            } else {
                None
            }
        } else {
            None
        }
    }
    
    /// Initialize standard file descriptors (stdin, stdout, stderr)
    pub fn init_std_fds(&mut self, stdin_handle: u32, stdout_handle: u32, stderr_handle: u32) {
        // WASI convention: fd 0 = stdin, fd 1 = stdout, fd 2 = stderr
        self.fd_to_handle.insert(0, stdin_handle);
        self.fd_to_handle.insert(1, stdout_handle);
        self.fd_to_handle.insert(2, stderr_handle);
        
        // Remove std fds from free list
        self.free_fds.retain(|&fd| fd != 0 && fd != 1 && fd != 2);
    }
}

impl AbiModule for WasiPreview1Abi {
    fn name() -> &'static str {
        "wasi-preview1"
    }
    
    fn get_name(&self) -> String {
        Self::name().to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }
    
    fn handle_syscall(&mut self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        syscall_handler(self, trapframe)
    }

    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>
    ) -> Option<u8> {
        // WASI binaries are WebAssembly modules
        // Check for WASM magic bytes: 0x00 0x61 0x73 0x6D (\\0asm)
        let magic_score = match file_object.as_file() {
            Some(file_obj) => {
                let mut magic_buffer = [0u8; 4];
                file_obj.seek(SeekFrom::Start(0)).ok();
                match file_obj.read(&mut magic_buffer) {
                    Ok(bytes_read) if bytes_read >= 4 => {
                        if magic_buffer == [0x00, 0x61, 0x73, 0x6D] { // \0asm
                            30 // Basic WASM format compatibility
                        } else {
                            return None; // Not a WASM file
                        }
                    }
                    _ => return None
                }
            }
            None => return None
        };
        
        let mut confidence = magic_score;
        
        // Check WASM version (should be 1 for MVP/Preview 1)
        if let Some(file_obj) = file_object.as_file() {
            let mut version_buffer = [0u8; 4];
            file_obj.seek(SeekFrom::Start(4)).ok();
            match file_obj.read(&mut version_buffer) {
                Ok(bytes_read) if bytes_read == 4 => {
                    if version_buffer == [0x01, 0x00, 0x00, 0x00] { // Version 1
                        confidence += 30;
                    }
                }
                _ => {}
            }
        }
        
        // File path hints
        if file_path.ends_with(".wasm") || file_path.contains("wasi") {
            confidence += 20;
        }
        
        // ABI inheritance bonus
        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence += 20;
            }
        }
        
        Some(confidence.min(100))
    }

    fn execute_binary(
        &self,
        _file_object: &crate::object::KernelObject,
        _argv: &[&str],
        _envp: &[&str],
        _task: &mut crate::task::Task,
        _trapframe: &mut Trapframe
    ) -> Result<(), &'static str> {
        // WASM execution requires a WebAssembly runtime
        // This is a placeholder for future WASM runtime integration
        Err("WASM execution not yet implemented")
    }

    fn initialize_from_existing_handles(&mut self, task: &mut crate::task::Task) -> Result<(), &'static str> {
        // Close all handles when switching to WASI ABI
        task.handle_table.close_all();
        Ok(())
    }
    
    fn get_default_cwd(&self) -> &str {
        "/" // WASI uses root as default
    }
}

// WASI Preview 1 System Calls
// Starting with basic file and process operations

/// fd_close - Close a file descriptor
fn sys_fd_close(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // fd: u32 (arg0)
    // Returns: errno
    0 // Success (ERRNO_SUCCESS)
}

/// fd_write - Write to a file descriptor
fn sys_fd_write(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // fd: u32 (arg0)
    // iovs_ptr: u32 (arg1) - pointer to iovec array
    // iovs_len: u32 (arg2) - number of iovecs
    // nwritten_ptr: u32 (arg3) - output: bytes written
    // Returns: errno
    0 // Success (ERRNO_SUCCESS)
}

/// fd_read - Read from a file descriptor
fn sys_fd_read(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // fd: u32 (arg0)
    // iovs_ptr: u32 (arg1) - pointer to iovec array
    // iovs_len: u32 (arg2) - number of iovecs
    // nread_ptr: u32 (arg3) - output: bytes read
    // Returns: errno
    0 // Success (ERRNO_SUCCESS)
}

/// proc_exit - Terminate the process
fn sys_proc_exit(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // rval: u32 (arg0) - exit code
    // This function does not return
    0
}

/// environ_sizes_get - Get environment variable sizes
fn sys_environ_sizes_get(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // environc_ptr: u32 (arg0) - output: number of environment variables
    // environ_buf_size_ptr: u32 (arg1) - output: size of environment buffer
    // Returns: errno
    0 // Success (ERRNO_SUCCESS)
}

/// environ_get - Get environment variables
fn sys_environ_get(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // environ_ptr: u32 (arg0) - output buffer for pointers
    // environ_buf_ptr: u32 (arg1) - output buffer for strings
    // Returns: errno
    0 // Success (ERRNO_SUCCESS)
}

/// args_sizes_get - Get command line argument sizes
fn sys_args_sizes_get(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // argc_ptr: u32 (arg0) - output: number of arguments
    // argv_buf_size_ptr: u32 (arg1) - output: size of argument buffer
    // Returns: errno
    0 // Success (ERRNO_SUCCESS)
}

/// args_get - Get command line arguments
fn sys_args_get(_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe) -> usize {
    // argv_ptr: u32 (arg0) - output buffer for pointers
    // argv_buf_ptr: u32 (arg1) - output buffer for strings
    // Returns: errno
    0 // Success (ERRNO_SUCCESS)
}

syscall_table! {
    Invalid = 0 => |_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe| {
        0
    },
    // WASI Preview 1 system calls
    // Numbers based on WASI Preview 1 specification
    ArgsGet = 1 => sys_args_get,
    ArgsSizesGet = 2 => sys_args_sizes_get,
    EnvironGet = 3 => sys_environ_get,
    EnvironSizesGet = 4 => sys_environ_sizes_get,
    ProcExit = 5 => sys_proc_exit,
    FdClose = 6 => sys_fd_close,
    FdRead = 8 => sys_fd_read,
    FdWrite = 10 => sys_fd_write,
}

fn register_wasi_preview1_abi() {
    register_abi!(WasiPreview1Abi);
}

early_initcall!(register_wasi_preview1_abi);
