//! Simplified Event System Calls
//! 
//! This module provides minimal system call stubs for event-related operations.
//! Most event handling is now delegated to ABI modules.

use crate::arch::Trapframe;

/// Placeholder for event-related system calls
/// 
/// These system calls now return error codes indicating that
/// event handling should be performed through the ABI layer.

/// Send an event to a task (placeholder)
pub fn sys_send_task_event(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle event sending
    38 // ENOSYS
}

/// Set event action for a task (placeholder)
pub fn sys_set_event_action(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle event actions
    38 // ENOSYS
}

/// Subscribe to event channel (placeholder)
pub fn sys_subscribe_channel(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle channel subscriptions
    38 // ENOSYS
}

/// Unsubscribe from event channel (placeholder)
pub fn sys_unsubscribe_channel(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle channel unsubscriptions
    38 // ENOSYS
}

/// Block/unblock event delivery (placeholder)
pub fn sys_block_events(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle event blocking
    38 // ENOSYS
}

/// Get pending event count (placeholder)
pub fn sys_get_pending_events(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle pending events
    38 // ENOSYS
}

/// Check if has pending events (placeholder)
pub fn sys_has_pending_events(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle event checking
    38 // ENOSYS
}

/// Send event with generic payload (placeholder)
pub fn sys_send_event_generic(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle generic event sending
    38 // ENOSYS
}

/// Join process group (placeholder)
pub fn sys_join_process_group(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle process group operations
    38 // ENOSYS
}

/// Leave process group (placeholder)
pub fn sys_leave_process_group(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle process group operations
    38 // ENOSYS
}

/// Broadcast event (placeholder)
pub fn sys_broadcast_event(_trapframe: &mut Trapframe) -> usize {
    // Return ENOSYS (function not implemented)
    // ABI modules should handle event broadcasting
    38 // ENOSYS
}
