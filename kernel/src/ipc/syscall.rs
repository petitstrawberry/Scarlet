//! IPC system calls
//! 
//! This module provides system call implementations for IPC operations
//! such as pipe creation, message passing, and shared memory.

use crate::{
    arch::Trapframe,
    task::mytask,
    ipc::pipe::UnidirectionalPipe,
};

/// sys_pipe - Create a pipe pair
/// 
/// Creates a unidirectional pipe with read and write ends.
/// 
/// Arguments:
/// - pipefd: Pointer to an array of 2 integers where file descriptors will be stored
///   - pipefd[0] will contain the read end file descriptor
///   - pipefd[1] will contain the write end file descriptor
/// 
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_pipe(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let pipefd_ptr = trapframe.get_arg(0);
    
    // Increment PC to avoid infinite loop if pipe creation fails
    trapframe.increment_pc_next(task);
    
    // Translate the pointer to get access to the pipefd array
    let pipefd_vaddr = match task.vm_manager.translate_vaddr(pipefd_ptr) {
        Some(addr) => addr as *mut u32,
        None => return usize::MAX, // Invalid pointer
    };
    
    // Create pipe pair with default buffer size (4KB)
    const DEFAULT_PIPE_BUFFER_SIZE: usize = 4096;
    let (read_obj, write_obj) = UnidirectionalPipe::create_pair(DEFAULT_PIPE_BUFFER_SIZE);
    
    // Insert into handle table with explicit IPC metadata
    use crate::object::handle::{HandleMetadata, HandleType, AccessMode};
    
    let read_metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadOnly,
        special_semantics: None,
    };
    
    let write_metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::WriteOnly,
        special_semantics: None,
    };
    
    let read_handle = match task.handle_table.insert_with_metadata(read_obj, read_metadata) {
        Ok(handle) => handle,
        Err(_) => return usize::MAX, // Too many open handles
    };
    
    let write_handle = match task.handle_table.insert_with_metadata(write_obj, write_metadata) {
        Ok(handle) => handle,
        Err(_) => {
            // Clean up the read handle if write handle allocation fails
            let _ = task.handle_table.remove(read_handle);
            return usize::MAX;
        }
    };
    
    // Write the handles to user space
    unsafe {
        *pipefd_vaddr = read_handle;
        *pipefd_vaddr.add(1) = write_handle;
    }
    
    0 // Success
}

/// sys_pipe2 - Create a pipe pair with flags (future implementation)
/// 
/// Extended version of sys_pipe that supports flags for controlling
/// pipe behavior (e.g., O_NONBLOCK, O_CLOEXEC).
pub fn sys_pipe2(trapframe: &mut Trapframe) -> usize {
    let _pipefd_ptr = trapframe.get_arg(0);
    let _flags = trapframe.get_arg(1);
    
    // For now, just call the basic sys_pipe implementation
    // TODO: Implement flag handling
    sys_pipe(trapframe)
}
