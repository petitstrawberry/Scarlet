//! Task Event System Calls
//! 
//! This module provides system call implementations for the task event system,
//! allowing user-space programs to send events/signals and manage event handlers.

use crate::{
    arch::Trapframe,
    task::{
        mytask,
        events::{
            TaskEventType, EventAction, EventTarget,
            send_terminate_event, send_kill_event, send_interrupt_event, 
            send_user_event, send_timer_event, send_pipe_broken_event, 
            send_io_ready_event, set_event_action, 
            block_task_events, unblock_task_events, has_pending_events, 
            get_pending_event_count, send_event_to_target, broadcast_event,
            subscribe_to_channel, unsubscribe_from_channel, 
            add_task_to_process_group, remove_task_from_process_group
        }
    }
};
use alloc::{format, vec};

/// Send a signal/event to a target task
/// 
/// # Arguments
/// * arg0: target_task_id (usize)
/// * arg1: event_type (u32) - maps to TaskEventType
/// * arg2: user_data (optional, depending on event type)
/// 
/// # Returns
/// * 0 on success
/// * usize::MAX (-1) on error
pub fn sys_task_send_event(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let target_task_id = trapframe.get_arg(0);
    let event_type_raw = trapframe.get_arg(1) as u32;
    let user_data = trapframe.get_arg(2) as u32;
    
    trapframe.increment_pc_next(task);
    
    let source_task_id = Some(task.get_id());
    
    let result = match event_type_raw {
        1 => send_terminate_event(target_task_id, source_task_id),
        2 => send_kill_event(target_task_id, source_task_id),
        3 => send_interrupt_event(target_task_id, source_task_id),
        4 => send_user_event(user_data, target_task_id, source_task_id, None),
        5 => send_timer_event(target_task_id, user_data),
        6 => send_pipe_broken_event(target_task_id),
        7 => send_io_ready_event(target_task_id, user_data as i32),
        _ => Err("Invalid event type"),
    };
    
    match result {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

/// Set event action for current task
/// 
/// # Arguments
/// * arg0: event_type (u32) - maps to TaskEventType
/// * arg1: action_type (u32) - maps to EventAction
/// 
/// # Returns
/// * 0 on success
/// * usize::MAX (-1) on error
pub fn sys_task_set_event_action(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let event_type_raw = trapframe.get_arg(0) as u32;
    let action_type = trapframe.get_arg(1) as u32;
    
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    
    // Convert raw event type to TaskEventType
    let event_type = match event_type_raw {
        1 => TaskEventType::Terminate,
        2 => TaskEventType::Kill,
        3 => TaskEventType::Interrupt,
        4 => {
            let user_id = trapframe.get_arg(2) as u32;
            TaskEventType::User(user_id)
        },
        5 => TaskEventType::Timer,
        6 => TaskEventType::Suspend,
        7 => TaskEventType::Resume,
        8 => TaskEventType::ChildStateChange,
        9 => TaskEventType::IoReady,
        10 => TaskEventType::PipeBroken,
        11 => TaskEventType::WindowChange,
        _ => return usize::MAX, // Invalid event type
    };
    
    // Convert action type to EventAction
    let action = match action_type {
        0 => EventAction::Ignore,
        1 => EventAction::Terminate,
        2 => EventAction::Suspend,
        3 => EventAction::Resume,
        4 => EventAction::Default,
        _ => return usize::MAX, // Invalid action type
    };
    
    set_event_action(task_id, event_type, action);
    0
}

/// Block or unblock event delivery for current task
/// 
/// # Arguments
/// * arg0: block (1) or unblock (0)
/// 
/// # Returns
/// * 0 on success
pub fn sys_task_block_events(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let should_block = trapframe.get_arg(0) != 0;
    
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    
    if should_block {
        block_task_events(task_id);
    } else {
        unblock_task_events(task_id);
    }
    
    0
}

/// Get pending event count for current task
/// 
/// # Returns
/// * Number of pending events
pub fn sys_task_get_pending_events(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    get_pending_event_count(task_id)
}

/// Check if current task has pending events
/// 
/// # Returns
/// * 1 if has pending events, 0 otherwise
pub fn sys_task_has_pending_events(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    if has_pending_events(task_id) { 1 } else { 0 }
}

/// Generic event sending with target specification
/// 
/// # Arguments
/// * arg0: event_type (u32)
/// * arg1: target_type (u32) - 0=Task, 1=ProcessGroup, 2=Broadcast, 3=TaskList, 4=Channel
/// * arg2: target_value (usize) - task_id, group_id, or pointer to channel name/task list
/// 
/// # Returns
/// * Number of tasks the event was sent to, or usize::MAX (-1) on error
pub fn sys_task_send_event_generic(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let event_type_raw = trapframe.get_arg(0) as u32;
    let target_type = trapframe.get_arg(1) as u32;
    let target_value = trapframe.get_arg(2);
    
    trapframe.increment_pc_next(task);
    
    let source_task_id = Some(task.get_id());
    
    // Convert raw event type to TaskEventType
    let event_type = match event_type_raw {
        1 => TaskEventType::Terminate,
        2 => TaskEventType::Kill,
        3 => TaskEventType::Interrupt,
        4 => {
            let user_id = trapframe.get_arg(3) as u32;
            TaskEventType::User(user_id)
        },
        5 => TaskEventType::Timer,
        6 => TaskEventType::Suspend,
        7 => TaskEventType::Resume,
        8 => TaskEventType::ChildStateChange,
        9 => TaskEventType::IoReady,
        10 => TaskEventType::PipeBroken,
        11 => TaskEventType::WindowChange,
        _ => return usize::MAX, // Invalid event type
    };
    
    // Create target specification
    let target = match target_type {
        0 => EventTarget::Task(target_value), // Task
        1 => EventTarget::ProcessGroup(target_value), // ProcessGroup
        2 => EventTarget::Broadcast, // Broadcast
        3 => {
            // TaskList - for now, we'll support single task
            EventTarget::TaskList(vec![target_value])
        },
        4 => {
            // Channel - get channel name from user space
            // For simplicity, we'll use a simple channel naming scheme
            let channel_name = format!("channel_{}", target_value);
            EventTarget::Channel(channel_name)
        },
        _ => return usize::MAX, // Invalid target type
    };
    
    match send_event_to_target(event_type, target, source_task_id) {
        Ok(sent_count) => sent_count,
        Err(_) => usize::MAX, // -1
    }
}

/// Subscribe to an event channel
/// 
/// # Arguments
/// * arg0: channel_id (u32) - simple numeric channel ID
/// 
/// # Returns
/// * 0 on success, usize::MAX (-1) on error
pub fn sys_task_subscribe_channel(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let channel_id = trapframe.get_arg(0) as u32;
    
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    let channel_name = format!("channel_{}", channel_id);
    
    match subscribe_to_channel(task_id, channel_name) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

/// Unsubscribe from an event channel
/// 
/// # Arguments
/// * arg0: channel_id (u32) - simple numeric channel ID
/// 
/// # Returns
/// * 0 on success, usize::MAX (-1) on error
pub fn sys_task_unsubscribe_channel(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let channel_id = trapframe.get_arg(0) as u32;
    
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    let channel_name = format!("channel_{}", channel_id);
    
    match unsubscribe_from_channel(task_id, &channel_name) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

/// Join a process group
/// 
/// # Arguments
/// * arg0: group_id (usize)
/// 
/// # Returns
/// * 0 on success, usize::MAX (-1) on error
pub fn sys_task_join_process_group(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let group_id = trapframe.get_arg(0);
    
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    
    match add_task_to_process_group(task_id, group_id) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

/// Leave a process group
/// 
/// # Arguments
/// * arg0: group_id (usize)
/// 
/// # Returns
/// * 0 on success, usize::MAX (-1) on error
pub fn sys_task_leave_process_group(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let group_id = trapframe.get_arg(0);
    
    trapframe.increment_pc_next(task);
    
    let task_id = task.get_id();
    
    match remove_task_from_process_group(task_id, group_id) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

/// Broadcast an event to all tasks
/// 
/// # Arguments
/// * arg0: event_type (u32)
/// 
/// # Returns
/// * Number of tasks the event was sent to, or usize::MAX (-1) on error
pub fn sys_task_broadcast_event(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let event_type_raw = trapframe.get_arg(0) as u32;
    
    trapframe.increment_pc_next(task);
    
    let source_task_id = Some(task.get_id());
    
    // Convert raw event type to TaskEventType
    let event_type = match event_type_raw {
        1 => TaskEventType::Terminate,
        2 => TaskEventType::Kill,
        3 => TaskEventType::Interrupt,
        4 => {
            let user_id = trapframe.get_arg(1) as u32;
            TaskEventType::User(user_id)
        },
        5 => TaskEventType::Timer,
        6 => TaskEventType::Suspend,
        7 => TaskEventType::Resume,
        8 => TaskEventType::ChildStateChange,
        9 => TaskEventType::IoReady,
        10 => TaskEventType::PipeBroken,
        11 => TaskEventType::WindowChange,
        _ => return usize::MAX, // Invalid event type
    };
    
    match broadcast_event(event_type, source_task_id) {
        Ok(sent_count) => sent_count,
        Err(_) => usize::MAX, // -1
    }
}
