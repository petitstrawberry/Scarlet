//! Handle introspection system call
//!
//! Provides sys_handle_query for KernelObject type and capability discovery

use crate::{
    arch::Trapframe,
    object::{
        handle::HandleMetadata, handle::HandleType, handle::StandardInputOutput,
        introspection::KernelObjectInfo,
    },
    task::mytask,
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
    trapframe.increment_pc_next(&task);

    // Translate the pointer to get access to the info structure
    let info_vaddr = match task.vm_manager.translate_to_kva(info_ptr) {
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

    trapframe.increment_pc_next(&task);

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

/// Close a handle (sys_handle_close)
///
/// This system call closes a handle and removes it from the handle table.
///
/// # Arguments
/// - handle: The handle to close
///
/// # Returns
/// - 0 on success
/// - usize::MAX on error (invalid handle)
pub fn sys_handle_close(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    trapframe.increment_pc_next(&task);

    if task.handle_table.remove(handle).is_some() {
        0 // Success
    } else {
        usize::MAX // Invalid handle
    }
}

/// Duplicate a handle (sys_handle_duplicate)
///
/// This system call creates a new handle that refers to the same kernel object
/// as the original handle.
///
/// # Arguments
/// - handle: The handle to duplicate
///
/// # Returns
/// - New handle number on success
/// - usize::MAX on error (invalid handle, handle table full)
pub fn sys_handle_duplicate(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    trapframe.increment_pc_next(&task);

    // Duplicate using object-specific dup semantics where available.
    if let Some((kernel_obj, metadata)) = task.handle_table.clone_for_dup(handle) {
        match task.handle_table.insert_with_metadata(kernel_obj, metadata) {
            Ok(new_handle) => new_handle as usize,
            Err(_) => usize::MAX, // Handle table full
        }
    } else {
        usize::MAX // Invalid handle
    }
}

/// Decode HandleType from raw value
fn decode_handle_type(raw: usize) -> Option<HandleType> {
    match raw {
        0 => Some(HandleType::Regular),
        1 => Some(HandleType::IpcChannel),
        2 => Some(HandleType::StandardInputOutput(StandardInputOutput::Stdin)),
        3 => Some(HandleType::StandardInputOutput(StandardInputOutput::Stdout)),
        4 => Some(HandleType::StandardInputOutput(StandardInputOutput::Stderr)),
        5 => Some(HandleType::EventChannel),
        6 => Some(HandleType::EventSubscription),
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
        HandleType::EventChannel => 5,
        HandleType::EventSubscription => 6,
    }
}

/// sys_handle_control - Perform control operations on a handle
///
/// This system call allows user space to perform device-specific control
/// operations on a handle, similar to ioctl operations in POSIX systems.
///
/// # Arguments
/// - handle: The handle to perform the control operation on
/// - command: The control command identifier
/// - arg: Command-specific argument (often a pointer to data)
///
/// # Returns
/// - i32 value on success (command-specific)
/// - usize::MAX on error
pub fn sys_handle_control(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    let command = trapframe.get_arg(1) as u32;
    let arg = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(&task);

    // Pin the kernel object for the duration of this control operation.
    let kernel_object = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX,
    };

    sys_handle_control_for_object(&kernel_object, command, arg)
}

fn sys_handle_control_for_object(
    kernel_object: &crate::object::KernelObject,
    command: u32,
    arg: usize,
) -> usize {
    #[cfg(feature = "network")]
    if command == crate::network::socket::socket_ctl::SCTL_SOCKET_TAKE_ERROR {
        let Some(socket) = kernel_object.as_socket() else {
            return usize::MAX;
        };
        return socket
            .take_pending_error()
            .as_ref()
            .map(crate::network::socket::socket_error_to_native_errno)
            .unwrap_or(0) as usize;
    }

    sys_handle_control_for_capabilities(
        kernel_object.as_selectable(),
        kernel_object.as_control(),
        command,
        arg,
    )
}

fn sys_handle_control_for_capabilities(
    selectable: Option<&dyn crate::object::capability::Selectable>,
    control_ops: Option<&dyn crate::object::capability::ControlOps>,
    command: u32,
    arg: usize,
) -> usize {
    const HCTL_SET_NONBLOCKING: u32 = 0x5353_0007;
    const HCTL_GET_NONBLOCKING: u32 = 0x5353_000B;

    // Non-blocking mode is a generic Selectable capability rather than a
    // socket-specific operation. Keep the legacy command values so existing
    // user-space callers continue to work for sockets, pipes, TTYs, and PTYs.
    // Sockets which implement the legacy control operation but are not
    // Selectable must still reach their ControlOps implementation below.
    match command {
        HCTL_SET_NONBLOCKING => {
            if let Some(selectable) = selectable {
                selectable.set_nonblocking(arg != 0);
                return 0;
            }
        }
        HCTL_GET_NONBLOCKING => {
            if let Some(selectable) = selectable {
                return usize::from(selectable.is_nonblocking());
            }
        }
        _ => {}
    }

    // Perform the control operation using the ControlOps capability
    let result = match control_ops {
        Some(control_ops) => control_ops.control(command, arg),
        None => Err("Control operations not supported on this object"),
    };

    // Convert result to usize for system call return value
    match result {
        Ok(value) => value as usize,
        Err(_) => usize::MAX,
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::{sys_handle_control_for_capabilities, sys_handle_control_for_object};
    use crate::network::icmp::IcmpLayer;
    use crate::network::socket::socket_ctl;
    use crate::object::KernelObject;
    use crate::object::capability::selectable::{ReadyInterest, SelectWaitOutcome};
    use crate::object::capability::{ControlOps, Selectable};

    struct ControlOnlyNonblocking {
        nonblocking: AtomicBool,
    }

    impl ControlOps for ControlOnlyNonblocking {
        fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
            match command {
                socket_ctl::SCTL_SOCKET_SET_NONBLOCK => {
                    self.nonblocking.store(arg != 0, Ordering::SeqCst);
                    Ok(0)
                }
                socket_ctl::SCTL_SOCKET_GET_NONBLOCK => {
                    Ok(self.nonblocking.load(Ordering::SeqCst) as i32)
                }
                _ => Err("unsupported control command"),
            }
        }
    }

    struct SelectableAndControl {
        selectable_nonblocking: AtomicBool,
        control_calls: AtomicUsize,
    }

    impl Selectable for SelectableAndControl {
        fn wait_until_ready(
            &self,
            _interest: ReadyInterest,
            _trapframe: &mut crate::arch::Trapframe,
            _timeout_ns: Option<u64>,
            _min_wait_ns: u64,
        ) -> SelectWaitOutcome {
            SelectWaitOutcome::Ready
        }

        fn set_nonblocking(&self, enabled: bool) {
            self.selectable_nonblocking.store(enabled, Ordering::SeqCst);
        }

        fn is_nonblocking(&self) -> bool {
            self.selectable_nonblocking.load(Ordering::SeqCst)
        }
    }

    impl ControlOps for SelectableAndControl {
        fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(99)
        }
    }

    #[test_case]
    fn nonblocking_control_falls_back_to_control_ops_without_selectable() {
        let control_only = ControlOnlyNonblocking {
            nonblocking: AtomicBool::new(false),
        };

        assert_eq!(
            sys_handle_control_for_capabilities(
                None,
                Some(&control_only),
                socket_ctl::SCTL_SOCKET_SET_NONBLOCK,
                1,
            ),
            0
        );
        assert_eq!(
            sys_handle_control_for_capabilities(
                None,
                Some(&control_only),
                socket_ctl::SCTL_SOCKET_GET_NONBLOCK,
                0,
            ),
            1
        );
    }

    #[test_case]
    fn nonblocking_control_prefers_selectable_over_control_ops() {
        let selectable_and_control = SelectableAndControl {
            selectable_nonblocking: AtomicBool::new(false),
            control_calls: AtomicUsize::new(0),
        };

        assert_eq!(
            sys_handle_control_for_capabilities(
                Some(&selectable_and_control),
                Some(&selectable_and_control),
                socket_ctl::SCTL_SOCKET_SET_NONBLOCK,
                1,
            ),
            0
        );
        assert_eq!(
            sys_handle_control_for_capabilities(
                Some(&selectable_and_control),
                Some(&selectable_and_control),
                socket_ctl::SCTL_SOCKET_GET_NONBLOCK,
                0,
            ),
            1
        );
        assert_eq!(
            selectable_and_control.control_calls.load(Ordering::SeqCst),
            0
        );
    }

    #[test_case]
    fn icmp_nonblocking_control_falls_back_when_not_selectable() {
        let icmp = IcmpLayer::new().create_socket();
        let object = KernelObject::from_socket_object(icmp);

        assert!(object.as_selectable().is_none());
        assert_eq!(
            sys_handle_control_for_object(&object, socket_ctl::SCTL_SOCKET_SET_NONBLOCK, 1),
            0
        );
        assert_eq!(
            sys_handle_control_for_object(&object, socket_ctl::SCTL_SOCKET_GET_NONBLOCK, 0),
            1
        );
        assert_eq!(
            sys_handle_control_for_object(&object, socket_ctl::SCTL_SOCKET_SET_NONBLOCK, 0),
            0
        );
        assert_eq!(
            sys_handle_control_for_object(&object, socket_ctl::SCTL_SOCKET_GET_NONBLOCK, 0),
            0
        );
    }
}
