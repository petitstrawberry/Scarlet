//! IPC system calls
//!
//! This module provides system call implementations for IPC operations
//! such as pipe creation, message passing, and shared memory.

use crate::{
    arch::Trapframe,
    ipc::event::{
        Event, EventContent, EventManager, EventPayload, EventPriority, ProcessControlType,
    },
    ipc::pipe::UnidirectionalPipe,
    ipc::shared_memory::SharedMemory,
    library::std::string::parse_c_string_from_userspace,
    object::KernelObject,
    object::capability::EventSubscriber,
    task::mytask,
};
use alloc::{string::ToString, sync::Arc};

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
    use crate::object::handle::{AccessMode, HandleMetadata, HandleType};

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

    let read_handle = match task
        .handle_table
        .insert_with_metadata(read_obj, read_metadata)
    {
        Ok(handle) => handle,
        Err(_) => return usize::MAX, // Too many open handles
    };

    let write_handle = match task
        .handle_table
        .insert_with_metadata(write_obj, write_metadata)
    {
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
        Some(KernelObject::EventSubscription(sub)) => (
            sub.channel_name().to_string(),
            sub.subscription_id().to_string(),
        ),
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
        EventContent::Custom {
            namespace: "user".into(),
            event_id,
        },
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

    let ko = match task.handle_table.get(sub_handle) {
        Some(obj) => obj,
        None => return usize::MAX,
    };
    let sub = match ko.as_event_subscription() {
        Some(s) => s,
        None => return usize::MAX,
    };

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
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    let target = trapframe.get_arg(0) as u32;
    let kind = trapframe.get_arg(1) as u32;
    let reliable = trapframe.get_arg(2) as u32 != 0;
    let prio_raw = trapframe.get_arg(3) as u32;
    trapframe.increment_pc_next(task);

    let priority = match prio_raw {
        1 => EventPriority::Low,
        3 => EventPriority::High,
        4 => EventPriority::Critical,
        _ => EventPriority::Normal,
    };

    let event = if kind >= 1000 {
        Event::direct_custom(
            target,
            "user".into(),
            kind - 1000,
            priority,
            reliable,
            EventPayload::Empty,
        )
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
    match mgr.send_event(event) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

/// sys_shared_memory_create - Create a shared memory object
///
/// Creates a shared memory region that can be mapped into multiple processes.
///
/// Arguments:
/// - size: Size of the shared memory region in bytes
/// - permissions: Access permissions (read/write/execute flags)
///   - 0x1: Read permission
///   - 0x2: Write permission
///   - 0x4: Execute permission
///
/// Returns:
/// - Handle to the shared memory object on success
/// - usize::MAX on error
pub fn sys_shared_memory_create(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let size = trapframe.get_arg(0);
    let permissions = trapframe.get_arg(1);

    // Increment PC to avoid infinite loop if creation fails
    trapframe.increment_pc_next(task);

    // Validate size (must be non-zero and reasonable)
    if size == 0 || size > 1024 * 1024 * 1024 {
        // Max 1GB
        return usize::MAX;
    }

    // Create the shared memory object
    let shmem = match SharedMemory::new(size, permissions) {
        Ok(shmem) => shmem,
        Err(_) => return usize::MAX,
    };

    // Wrap in KernelObject
    let kernel_obj = KernelObject::from_shared_memory_object(Arc::new(shmem));

    // Insert into handle table with IPC metadata
    use crate::object::handle::{AccessMode, HandleMetadata, HandleType};

    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: if permissions & 0x3 == 0x3 {
            AccessMode::ReadWrite
        } else if permissions & 0x2 != 0 {
            AccessMode::WriteOnly
        } else {
            AccessMode::ReadOnly
        },
        special_semantics: None,
    };

    let handle = match task.handle_table.insert_with_metadata(kernel_obj, metadata) {
        Ok(handle) => handle,
        Err(_) => return usize::MAX, // Too many open handles
    };

    handle as usize
}

/// sys_shared_memory_resize - Resize a shared memory object
///
/// Arguments:
/// - handle: Handle to the shared memory object
/// - size: New size in bytes (will be page-aligned in kernel)
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_shared_memory_resize(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    let size = trapframe.get_arg(1);

    crate::println!("[sys_shared_memory_resize] handle={} size={}", handle, size);

    trapframe.increment_pc_next(task);

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            crate::println!("[sys_shared_memory_resize] handle not found");
            return usize::MAX;
        }
    };

    let shared_memory = match kernel_obj.as_shared_memory() {
        Some(obj) => obj,
        None => {
            crate::println!("[sys_shared_memory_resize] not a shared memory object");
            return usize::MAX;
        }
    };

    if let Err(e) = shared_memory.resize(size) {
        crate::println!("[sys_shared_memory_resize] resize failed: {}", e);
        return usize::MAX;
    }

    crate::println!("[sys_shared_memory_resize] SUCCESS new_size={}", shared_memory.size());
    0
}

/// sys_socket_send_handle - Send a kernel object handle through a socket
///
/// Transfers a kernel object (like SharedMemoryObject) to another task
/// through a connected socket, similar to Unix SCM_RIGHTS functionality.
/// Uses dup() semantics - the handle is duplicated, not moved, so both
/// sender and receiver will have independent references to the same object.
///
/// Arguments:
/// - socket_handle: Handle to the connected socket
/// - object_handle: Handle to the kernel object to send (remains valid after send)
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_socket_send_handle(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let socket_handle = trapframe.get_arg(0) as u32;
    let object_handle = trapframe.get_arg(1) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get the socket object (LocalSocket-only)
    let socket_obj = match task.handle_table.get(socket_handle) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX, // Invalid socket handle
    };

    use crate::network::local::LocalSocket;
    let local_socket = match LocalSocket::from_socket_object(&socket_obj) {
        Some(s) => s,
        None => return usize::MAX, // Not a LocalSocket
    };

    // Get the kernel object to send with dup semantics
    // Use clone_for_dup() to properly increment reference counts for objects like Pipes
    let object = match task.handle_table.clone_for_dup(object_handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid object handle
    };

    // Send the handle through the socket
    match local_socket.send_handle(object) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

/// sys_socket_recv_handle - Receive a kernel object handle from a socket
///
/// Receives a kernel object that was sent by a peer task through a
/// connected socket.
///
/// Arguments:
/// - socket_handle: Handle to the connected socket
///
/// Returns:
/// - Handle to the received kernel object on success
/// - usize::MAX on error (no handle available or other error)
pub fn sys_socket_recv_handle(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let socket_handle = trapframe.get_arg(0) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get the socket object (LocalSocket-only)
    let socket_obj = match task.handle_table.get(socket_handle) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX, // Invalid socket handle
    };

    // For LocalSocket, we provide blocking semantics: if the handle queue is empty,
    // block the task until a handle arrives or the peer is closed.
    use crate::network::local::LocalSocket;
    let local_socket = match LocalSocket::from_socket_object(&socket_obj) {
        Some(s) => s,
        None => return usize::MAX, // Not a LocalSocket
    };

    let object = match local_socket.recv_handle_blocking(task.get_id(), trapframe) {
        Ok(obj) => obj,
        Err(_) => return usize::MAX,
    };

    // Insert the received object into this task's handle table
    match task.handle_table.insert(object) {
        Ok(handle) => handle as usize,
        Err(_) => usize::MAX, // Too many open handles
    }
}

/// sys_socket_send_handle_and_data - Send a kernel object handle and data atomically
///
/// Sends a kernel object handle through a connected socket, along with data.
/// This ensures both the handle and data are available before waking the peer,
/// preventing race conditions in protocols like Wayland.
///
/// Arguments:
/// - socket_handle: Handle to the connected socket
/// - object_handle: Handle to the kernel object to send
/// - data_ptr: Pointer to the data buffer
/// - data_len: Length of the data buffer
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_socket_send_handle_and_data(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let socket_handle = trapframe.get_arg(0) as u32;
    let object_handle = trapframe.get_arg(1) as u32;
    let data_ptr = trapframe.get_arg(2);
    let data_len = trapframe.get_arg(3);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get the socket object (LocalSocket-only)
    let socket_obj = match task.handle_table.get(socket_handle) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX, // Invalid socket handle
    };

    use crate::network::local::LocalSocket;
    let local_socket = match LocalSocket::from_socket_object(&socket_obj) {
        Some(s) => s,
        None => return usize::MAX, // Not a LocalSocket
    };

    // Get the kernel object to send with dup semantics
    let object = match task.handle_table.clone_for_dup(object_handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid object handle
    };

    // Validate and translate data pointer
    if data_len == 0 {
        // No data to send, just send the handle
        match local_socket.send_handle(object) {
            Ok(()) => return 0,
            Err(_) => return usize::MAX,
        }
    }

    let data_addr = match task.vm_manager.translate_vaddr(data_ptr) {
        Some(addr) => addr,
        None => return usize::MAX, // Invalid pointer
    };

    // Limit data size to prevent DoS attacks
    const MAX_SEND_SIZE: usize = 65536; // 64 KB max
    let data_len = data_len.min(MAX_SEND_SIZE);

    // Read data from userspace
    let data = unsafe { core::slice::from_raw_parts(data_addr as *const u8, data_len) };

    // Send the handle and data atomically
    match local_socket.send_handle_and_data(object, data) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}

/// sys_socket_recv_handle_and_data - Receive a kernel object handle and data atomically
///
/// Receives both a kernel object handle and data in a single atomic operation.
/// This is the counterpart to send_handle_and_data.
///
/// Arguments:
/// - socket_handle: Handle to the connected socket
/// - handle_ptr: Pointer to store the received handle (output)
/// - data_ptr: Pointer to store the received data (output)
/// - max_data_len: Maximum amount of data to receive
///
/// Returns:
/// - Number of bytes received on success
/// - usize::MAX on error
pub fn sys_socket_recv_handle_and_data(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let socket_handle = trapframe.get_arg(0) as u32;
    let handle_ptr = trapframe.get_arg(1);
    let data_ptr = trapframe.get_arg(2);
    let max_data_len = trapframe.get_arg(3);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get the socket object (LocalSocket-only)
    let socket_obj = match task.handle_table.get(socket_handle) {
        Some(KernelObject::Socket(socket)) => socket.clone(),
        _ => return usize::MAX, // Invalid socket handle
    };

    use crate::network::local::LocalSocket;
    let local_socket = match LocalSocket::from_socket_object(&socket_obj) {
        Some(s) => s,
        None => return usize::MAX, // Not a LocalSocket
    };

    // Limit data size to prevent DoS attacks
    const MAX_RECV_SIZE: usize = 65536; // 64 KB max
    let max_data_len = max_data_len.min(MAX_RECV_SIZE);

    // Receive handle and data atomically
    let (object, data) = match local_socket.recv_handle_and_data(max_data_len) {
        Ok((h, d)) => (h, d),
        Err(_) => return usize::MAX,
    };

    // Insert the received object into this task's handle table
    let new_handle = match task.handle_table.insert(object) {
        Ok(h) => h,
        Err(_) => return usize::MAX, // Too many open handles
    };

    // Write the handle value to userspace
    if handle_ptr != 0 {
        let handle_addr = match task.vm_manager.translate_vaddr(handle_ptr) {
            Some(addr) => addr as *mut u32,
            None => return usize::MAX,
        };
        unsafe {
            *handle_addr = new_handle;
        }
    }

    // Write the data to userspace
    if !data.is_empty() && data_ptr != 0 {
        let data_addr = match task.vm_manager.translate_vaddr(data_ptr) {
            Some(addr) => addr as *mut u8,
            None => return usize::MAX,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), data_addr, data.len());
        }
    }

    data.len()
}
