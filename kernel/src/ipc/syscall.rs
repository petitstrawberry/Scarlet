//! IPC system calls
//! 
//! This module provides system call implementations for IPC operations
//! such as pipe creation, message passing, and shared memory.

use crate::{
    arch::Trapframe,
    task::mytask,
    ipc::pipe::UnidirectionalPipe,
    ipc::event::{EventManager, Event, EventContent, EventPayload, EventPriority, ProcessControlType},
    object::KernelObject,
    object::capability::EventSubscriber,
    library::std::string::parse_c_string_from_userspace,
};
use alloc::string::ToString;

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

// === Event IPC (Handle-based) ===

/// Create or open an event channel by name and return a handle (EventChannel)
/// 
/// Arguments:
/// - name_ptr: const char* (C-string) channel name
/// 
/// Returns: handle on success, usize::MAX on error
pub fn sys_event_channel_create(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let name_ptr = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);

    let name = match parse_c_string_from_userspace(task, name_ptr, 256) {
        Ok(s) => s,
        Err(_) => return usize::MAX,
    };

    let mgr = EventManager::get_manager();
    let ko = mgr.create_channel(name);
    match task.handle_table.insert(ko) {
        Ok(h) => h as usize,
        Err(_) => usize::MAX,
    }
}

/// Subscribe current task to a channel by name, returning an EventSubscription handle
/// 
/// Arguments:
/// - name_ptr: const char* (C-string) channel name
/// 
/// Returns: handle on success, usize::MAX on error
pub fn sys_event_subscribe(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let name_ptr = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);

    let name = match parse_c_string_from_userspace(task, name_ptr, 256) {
        Ok(s) => s,
        Err(_) => return usize::MAX,
    };

    let mgr = EventManager::get_manager();
    let ko = match mgr.create_subscription(name, task.get_id() as u32) {
        Ok(ko) => ko,
        Err(_) => return usize::MAX,
    };
    match task.handle_table.insert(ko) {
        Ok(h) => h as usize,
        Err(_) => usize::MAX,
    }
}

/// Unsubscribe and close an EventSubscription handle
/// 
/// Arguments:
/// - sub_handle: u32 subscription handle
/// 
/// Returns: 0 on success, usize::MAX on error
pub fn sys_event_unsubscribe(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    trapframe.increment_pc_next(task);

    // Get the object first to extract identifiers
    let (channel_name, subscription_id) = match task.handle_table.get(handle) {
        Some(KernelObject::EventSubscription(sub)) => {
            (sub.channel_name().to_string(), sub.subscription_id().to_string())
        }
        _ => return usize::MAX,
    };

    // Remove from channel registry via EventManager helper
    let mgr = EventManager::get_manager();
    let _ = mgr.remove_subscription_from_channel(&channel_name, &subscription_id);

    // Finally remove handle (drop Arc)
    match task.handle_table.remove(handle) {
        Some(_) => 0,
        None => usize::MAX,
    }
}

/// Publish a custom integer event to a channel using a channel handle
/// 
/// Arguments:
/// - channel_handle: u32 (EventChannel)
/// - event_id: u32 (custom event id in "user" namespace)
/// - payload: isize (integer payload)
/// 
/// Returns: 0 on success, usize::MAX on error
pub fn sys_event_publish(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let channel_handle = trapframe.get_arg(0) as u32;
    let event_id = trapframe.get_arg(1) as u32;
    let payload_val = trapframe.get_arg(2) as isize as i64;
    trapframe.increment_pc_next(task);

    let ko = match task.handle_table.get(channel_handle) {
        Some(obj) => obj,
        None => return usize::MAX,
    };

    let channel = match ko.as_event_channel() {
        Some(ch) => ch,
        None => return usize::MAX,
    };

    let ev = Event::channel(
        channel.name().to_string(),
        EventContent::Custom { namespace: "user".into(), event_id },
        false,
        EventPriority::Normal,
        EventPayload::Integer(payload_val),
    );

    match channel.broadcast_to_subscribers(ev) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

/// Register a filter on an EventSubscription handle
/// 
/// Arguments:
/// - sub_handle: u32
/// - handler_id: usize
/// - filter_kind: u32
///   - 0: All
///   - 1: Sender(param0)
///   - 2: EventId(param0)
///   - 3: DirectType(param0)
/// - param0: u32 (used depending on filter_kind)
pub fn sys_event_handler_register(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let sub_handle = trapframe.get_arg(0) as u32;
    let handler_id = trapframe.get_arg(1);
    let filter_kind = trapframe.get_arg(2) as u32;
    let param0 = trapframe.get_arg(3) as u32;
    trapframe.increment_pc_next(task);

    let ko = match task.handle_table.get(sub_handle) { Some(obj) => obj, None => return usize::MAX };
    let sub = match ko.as_event_subscription() { Some(s) => s, None => return usize::MAX };

    use crate::ipc::event::{EventFilter, EventTypeFilter};
    let filter = match filter_kind {
        0 => EventFilter::All,
        1 => EventFilter::Sender(param0),
        2 => EventFilter::EventId(param0),
        3 => EventFilter::EventType(EventTypeFilter::Direct(param0)),
        _ => EventFilter::All,
    };

    match sub.register_filter(filter, handler_id) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

/// Send a direct process control event to a target task
/// 
/// Arguments:
/// - target_tid: u32
/// - kind: u32 (0=Terminate,1=Kill,2=Stop,3=Continue,4=Interrupt,5=Quit,6=Hangup,7=ChildExit,8=PipeBroken,9=Alarm,10=IoReady,1000+=User(kind-1000))
/// - reliable: u32 (0/1)
/// - priority: u32 (1=Low,2=Normal,3=High,4=Critical)
pub fn sys_event_send_direct(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() { Some(task) => task, None => return usize::MAX };
    let target = trapframe.get_arg(0) as u32;
    let kind = trapframe.get_arg(1) as u32;
    let reliable = trapframe.get_arg(2) as u32 != 0;
    let prio_raw = trapframe.get_arg(3) as u32;
    trapframe.increment_pc_next(task);

    let priority = match prio_raw { 1 => EventPriority::Low, 3 => EventPriority::High, 4 => EventPriority::Critical, _ => EventPriority::Normal };

    let event = if kind >= 1000 {
        Event::direct_custom(target, "user".into(), kind - 1000, priority, reliable, EventPayload::Empty)
    } else {
        let ptype = match kind {
            0 => ProcessControlType::Terminate,
            1 => ProcessControlType::Kill,
            2 => ProcessControlType::Stop,
            3 => ProcessControlType::Continue,
            4 => ProcessControlType::Interrupt,
            5 => ProcessControlType::Quit,
            6 => ProcessControlType::Hangup,
            7 => ProcessControlType::ChildExit,
            8 => ProcessControlType::PipeBroken,
            9 => ProcessControlType::Alarm,
            10 => ProcessControlType::IoReady,
            _ => ProcessControlType::Terminate,
        };
        Event::direct_process_control(target, ptype, priority, reliable)
    };

    let mgr = EventManager::get_manager();
    match mgr.send_event(event) { Ok(()) => 0, Err(_) => usize::MAX }
}
