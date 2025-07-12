//! Handle introspection system call
//! 
//! Provides sys_handle_query for KernelObject type and capability discovery

use crate::{
    arch::Trapframe, 
    task::mytask, 
    object::{
        introspection::KernelObjectInfo,
        handle::HandleType,
        handle::StandardInputOutput,
        handle::HandleMetadata
    }
};

/// sys_handle_query - Get information about a KernelObject handle
/// 
/// This system call allows user space to discover the type and capabilities
/// of a KernelObject, enabling type-safe wrapper implementations.
/// 
/// # Arguments
/// - handle: The handle to query
/// - info_ptr: Pointer to KernelObjectInfo structure to fill
/// 
/// # Returns
/// - 0 on success
/// - usize::MAX on error
pub fn sys_handle_query(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let handle = trapframe.get_arg(0) as u32;
    let info_ptr = trapframe.get_arg(1);
    
    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);
    
    // Translate the pointer to get access to the info structure
    let info_vaddr = match task.vm_manager.translate_vaddr(info_ptr) {
        Some(addr) => addr as *mut KernelObjectInfo,
        None => return usize::MAX, // Invalid pointer
    };
    
    // Get object information
    match task.handle_table.get_object_info(handle) {
        Some(info) => {
            // Write the information to user space
            unsafe {
                *info_vaddr = info;
            }
            0 // Success
        }
        None => usize::MAX, // Invalid handle
    }
}

/// Change handle role after creation
/// 
/// Arguments:
/// - handle: Handle to modify
/// - new_role: New HandleType role
/// - flags: Additional flags
/// 
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_handle_set_role(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let handle = trapframe.get_arg(0) as u32;
    let new_role_raw = trapframe.get_arg(1);
    let _flags = trapframe.get_arg(2);
    
    trapframe.increment_pc_next(task);
    
    // Decode new role from raw value
    let new_role = match decode_handle_type(new_role_raw) {
        Some(role) => role,
        None => return usize::MAX, // Invalid role
    };
    
    // Get current metadata and verify handle exists
    let current_metadata = match task.handle_table.get_metadata(handle) {
        Some(meta) => meta.clone(),
        None => return usize::MAX, // Invalid handle
    };
    
    // Create new metadata with updated role
    let new_metadata = HandleMetadata {
        handle_type: new_role,
        access_mode: current_metadata.access_mode,
        special_semantics: current_metadata.special_semantics,
    };
    
    // Update metadata in handle table
    if let Err(_) = task.handle_table.update_metadata(handle, new_metadata) {
        return usize::MAX; // Update failed
    }
    
    0 // Success
}

/// Decode HandleType from raw value
fn decode_handle_type(raw: usize) -> Option<HandleType> {
    match raw {
        0 => Some(HandleType::Regular),
        1 => Some(HandleType::IpcChannel),
        2 => Some(HandleType::StandardInputOutput(StandardInputOutput::Stdin)),
        3 => Some(HandleType::StandardInputOutput(StandardInputOutput::Stdout)),
        4 => Some(HandleType::StandardInputOutput(StandardInputOutput::Stderr)),
        _ => None,
    }
}

/// Encode HandleType to raw value for user space
pub fn encode_handle_type(handle_type: &HandleType) -> usize {
    match handle_type {
        HandleType::Regular => 0,
        HandleType::IpcChannel => 1,
        HandleType::StandardInputOutput(StandardInputOutput::Stdin) => 2,
        HandleType::StandardInputOutput(StandardInputOutput::Stdout) => 3,
        HandleType::StandardInputOutput(StandardInputOutput::Stderr) => 4,
    }
}
