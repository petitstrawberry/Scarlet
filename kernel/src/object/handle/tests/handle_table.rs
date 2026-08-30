//! HandleTable tests

use super::super::{AccessMode, Handle, HandleMetadata, HandleTable, HandleType, KernelObject};
use super::mock::{MockFileObject, MockPipeObject};
use crate::device::gpu::GpuObject;
use crate::ipc::pipe::PipeObject;
use alloc::{format, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct ReentrantDropGpuObject {
    table: HandleTable,
    dropped: Arc<AtomicBool>,
    observed_open_count: Arc<AtomicUsize>,
}

impl GpuObject for ReentrantDropGpuObject {}

impl Drop for ReentrantDropGpuObject {
    fn drop(&mut self) {
        self.observed_open_count
            .store(self.table.open_count(), Ordering::SeqCst);
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[test_case]
fn test_handle_table_creation() {
    let table = HandleTable::new();
    assert_eq!(table.open_count(), 0);
    assert_eq!(table.active_handles().len(), 0);
    assert_eq!(table.free_handles_len(), HandleTable::MAX_HANDLES);
}

#[test_case]
fn test_handle_table_insert_and_get() {
    let table = HandleTable::new();
    let mock_file = Arc::new(MockFileObject::new(b"test".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);

    // Insert object
    let handle = table.insert(kernel_obj).unwrap();
    assert_eq!(handle, 0); // Should allocate the first available handle (0)
    assert_eq!(table.open_count(), 1);

    // Get object
    let retrieved_obj = table.get(handle).unwrap();
    assert!(retrieved_obj.as_stream().is_some());

    // Verify handle validity
    assert!(table.is_valid_handle(handle));
    assert!(!table.is_valid_handle(9999)); // Invalid handle
}

#[test_case]
fn test_handle_table_get_pins_object_across_shared_close() {
    let table = HandleTable::new();
    let shared_table = table.clone();
    let mock_file = Arc::new(MockFileObject::new(b"pinned".to_vec()));
    let handle = table.insert(KernelObject::File(mock_file.clone())).unwrap();

    let object = table.get(handle).unwrap();
    assert_eq!(Arc::strong_count(&mock_file), 3);

    drop(shared_table.remove(handle).unwrap());
    assert!(table.get(handle).is_none());
    assert_eq!(Arc::strong_count(&mock_file), 2);

    let replacement = Arc::new(MockFileObject::new(b"newest".to_vec()));
    let reused_handle = shared_table
        .insert(KernelObject::File(replacement.clone()))
        .unwrap();
    assert_eq!(reused_handle, handle);

    let replacement_object = table.get(reused_handle).unwrap();
    let mut old_bytes = [0u8; 6];
    let mut new_bytes = [0u8; 6];
    assert_eq!(object.as_stream().unwrap().read(&mut old_bytes).unwrap(), 6);
    assert_eq!(
        replacement_object
            .as_stream()
            .unwrap()
            .read(&mut new_bytes)
            .unwrap(),
        6
    );
    assert_eq!(&old_bytes, b"pinned");
    assert_eq!(&new_bytes, b"newest");

    drop(object);
    assert_eq!(Arc::strong_count(&mock_file), 1);
}

#[test_case]
fn test_handle_table_get_uses_arc_clone_not_dup_semantics() {
    let table = HandleTable::new();
    let pipe: Arc<dyn PipeObject> = Arc::new(MockPipeObject::new());
    let handle = table.insert(KernelObject::Pipe(Arc::clone(&pipe))).unwrap();

    let object = table.get(handle).unwrap();
    let retrieved_pipe = match object {
        KernelObject::Pipe(pipe) => pipe,
        _ => panic!("get should retain the pipe object type"),
    };

    assert!(Arc::ptr_eq(&pipe, &retrieved_pipe));
}

#[test_case]
fn test_handle_table_with_object_ref_does_not_clone_arc() {
    let table = HandleTable::new();
    let mock_file = Arc::new(MockFileObject::new(b"borrowed".to_vec()));
    let kernel_obj = KernelObject::File(mock_file.clone());

    let handle = table.insert(kernel_obj).unwrap();
    let count_before = Arc::strong_count(&mock_file);

    let has_stream = table
        .with_object_ref(handle, |object| object.as_stream().is_some())
        .unwrap();

    assert!(has_stream);
    assert_eq!(Arc::strong_count(&mock_file), count_before);
    assert!(table.with_object_ref(999, |_| true).is_none());
}

#[test_case]
fn test_handle_table_get_arc_clone_with_metadata() {
    let table = HandleTable::new();
    let mock_file: Arc<dyn crate::fs::FileObject> =
        Arc::new(MockFileObject::new(b"paired lookup".to_vec()));
    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadOnly,
        special_semantics: None,
    };
    let handle = table
        .insert_with_metadata(KernelObject::File(Arc::clone(&mock_file)), metadata)
        .unwrap();
    let count_before = Arc::strong_count(&mock_file);

    let (object, metadata) = table.get_arc_clone_with_metadata(handle).unwrap();
    assert!(matches!(&object, KernelObject::File(_)));
    assert_eq!(metadata.handle_type, HandleType::IpcChannel);
    assert_eq!(metadata.access_mode, AccessMode::ReadOnly);
    assert_eq!(Arc::strong_count(&mock_file), count_before + 1);

    drop(object);
    assert_eq!(Arc::strong_count(&mock_file), count_before);
    assert!(
        table
            .get_arc_clone_with_metadata(HandleTable::MAX_HANDLES as Handle)
            .is_none()
    );
    table.remove(handle);
    assert!(table.get_arc_clone_with_metadata(handle).is_none());
}

#[test_case]
fn test_handle_table_clone_for_dup_preserves_metadata() {
    let table = HandleTable::new();
    let pipe: Arc<dyn PipeObject> = Arc::new(MockPipeObject::new());
    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::WriteOnly,
        special_semantics: None,
    };
    let handle = table
        .insert_with_metadata(KernelObject::Pipe(Arc::clone(&pipe)), metadata)
        .unwrap();

    let (object, duplicated_metadata) = table.clone_for_dup(handle).unwrap();

    let duplicated_pipe = match object {
        KernelObject::Pipe(pipe) => pipe,
        _ => panic!("clone_for_dup should retain the pipe object type"),
    };
    assert!(
        !Arc::ptr_eq(&pipe, &duplicated_pipe),
        "clone_for_dup must use the pipe's custom clone semantics"
    );
    assert_eq!(duplicated_metadata.handle_type, HandleType::IpcChannel);
    assert_eq!(duplicated_metadata.access_mode, AccessMode::WriteOnly);
}

#[test_case]
fn test_handle_table_remove() {
    let table = HandleTable::new();
    let mock_file = Arc::new(MockFileObject::new(b"test".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);

    // Insert and then remove
    let handle = table.insert(kernel_obj).unwrap();
    assert_eq!(table.open_count(), 1);

    let removed_obj = table.remove(handle).unwrap();
    assert!(removed_obj.as_stream().is_some());
    assert_eq!(table.open_count(), 0);
    assert!(!table.is_valid_handle(handle));

    // Try to remove again (should return None)
    assert!(table.remove(handle).is_none());
}

#[test_case]
fn test_handle_table_multiple_objects() {
    let table = HandleTable::new();
    let mut handles = Vec::new();

    // Insert multiple objects
    for i in 0..10 {
        let mock_file = Arc::new(MockFileObject::new(format!("test {}", i).into_bytes()));
        let kernel_obj = KernelObject::File(mock_file);
        let handle = table.insert(kernel_obj).unwrap();
        handles.push(handle);
    }

    assert_eq!(table.open_count(), 10);
    assert_eq!(table.active_handles().len(), 10);

    // Verify all handles are valid
    for &handle in &handles {
        assert!(table.is_valid_handle(handle));
        assert!(table.get(handle).is_some());
    }

    // Remove some handles
    for &handle in &handles[0..5] {
        assert!(table.remove(handle).is_some());
    }

    assert_eq!(table.open_count(), 5);
    assert_eq!(table.active_handles().len(), 5);
}

#[test_case]
fn test_handle_table_close_all() {
    let table = HandleTable::new();

    // Insert multiple objects
    for i in 0..5 {
        let mock_file = Arc::new(MockFileObject::new(format!("test {}", i).into_bytes()));
        let kernel_obj = KernelObject::File(mock_file);
        let _ = table.insert(kernel_obj).unwrap();
    }

    assert_eq!(table.open_count(), 5);

    // Close all handles
    table.close_all();

    assert_eq!(table.open_count(), 0);
    assert_eq!(table.active_handles().len(), 0);
    assert_eq!(table.free_handles_len(), HandleTable::MAX_HANDLES);
}

#[test_case]
fn test_handle_table_close_all_drops_objects_after_unlock() {
    let table = HandleTable::new();
    let dropped = Arc::new(AtomicBool::new(false));
    let observed_open_count = Arc::new(AtomicUsize::new(usize::MAX));
    let object: Arc<dyn GpuObject> = Arc::new(ReentrantDropGpuObject {
        table: table.clone(),
        dropped: Arc::clone(&dropped),
        observed_open_count: Arc::clone(&observed_open_count),
    });

    table.insert(KernelObject::Gpu(object)).unwrap();
    table.close_all();

    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(observed_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(table.open_count(), 0);
}

#[test_case]
fn test_handle_table_limits() {
    let table = HandleTable::new();
    let mut handles = Vec::new();

    // Fill up the table
    for i in 0..HandleTable::MAX_HANDLES {
        let mock_file = Arc::new(MockFileObject::new(format!("test {}", i).into_bytes()));
        let kernel_obj = KernelObject::File(mock_file);
        let handle = table.insert(kernel_obj).unwrap();
        handles.push(handle);
    }

    assert_eq!(table.open_count(), HandleTable::MAX_HANDLES);

    // Try to insert one more (should fail)
    let mock_file = Arc::new(MockFileObject::new(b"overflow".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let result = table.insert(kernel_obj);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err(),
        "Too many open KernelObjects, limit reached"
    );
}

#[test_case]
fn test_handle_table_handle_reuse() {
    let table = HandleTable::new();

    // Insert object
    let mock_file1 = Arc::new(MockFileObject::new(b"first".to_vec()));
    let kernel_obj1 = KernelObject::File(mock_file1);
    let handle1 = table.insert(kernel_obj1).unwrap();

    // Remove object
    let _ = table.remove(handle1).unwrap();

    // Insert another object (should reuse the handle)
    let mock_file2 = Arc::new(MockFileObject::new(b"second".to_vec()));
    let kernel_obj2 = KernelObject::File(mock_file2);
    let handle2 = table.insert(kernel_obj2).unwrap();

    assert_eq!(handle1, handle2); // Handle should be reused
}

#[test_case]
fn test_handle_table_invalid_operations() {
    let table = HandleTable::new();

    // Try to get non-existent handle
    assert!(table.get(999).is_none());
    assert!(!table.is_valid_handle(999));

    // Try to remove non-existent handle
    assert!(table.remove(999).is_none());

    // Try to get handle beyond MAX_HANDLES
    assert!(table.get(HandleTable::MAX_HANDLES as Handle + 1).is_none());
    assert!(!table.is_valid_handle(HandleTable::MAX_HANDLES as Handle + 1));
}

#[test_case]
fn test_handle_table_stress_allocation() {
    let table = HandleTable::new();

    // Test rapid allocation/deallocation to ensure no memory leaks
    for _ in 0..100 {
        let mut handles = Vec::new();

        // Allocate up to 100 handles
        for i in 0..100 {
            let mock_file = Arc::new(MockFileObject::new(format!("stress_{}", i).into_bytes()));
            let kernel_obj = KernelObject::File(mock_file);
            let handle = table.insert(kernel_obj).unwrap();
            handles.push(handle);
        }

        // Free all handles
        for handle in handles {
            assert!(table.remove(handle).is_some());
        }

        assert_eq!(table.open_count(), 0);
    }
}

#[test_case]
fn test_handle_table_edge_cases() {
    let table = HandleTable::new();

    // Test edge case: handle 0 should be valid
    let mock_file = Arc::new(MockFileObject::new(b"handle_zero".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let handle = table.insert(kernel_obj).unwrap();
    assert_eq!(handle, 0);
    assert!(table.is_valid_handle(0));

    // Test edge case: MAX_HANDLES - 1 should be valid
    table.close_all();

    // Fill up to almost max
    for i in 0..(HandleTable::MAX_HANDLES - 1) {
        let mock_file = Arc::new(MockFileObject::new(format!("edge_{}", i).into_bytes()));
        let kernel_obj = KernelObject::File(mock_file);
        let _ = table.insert(kernel_obj).unwrap();
    }

    // Last insertion should succeed
    let mock_file = Arc::new(MockFileObject::new(b"last".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let last_handle = table.insert(kernel_obj).unwrap();
    assert!(table.is_valid_handle(last_handle));
    assert_eq!(table.open_count(), HandleTable::MAX_HANDLES);

    // Next insertion should fail
    let mock_file = Arc::new(MockFileObject::new(b"overflow".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    assert!(table.insert(kernel_obj).is_err());
}

#[test_case]
fn test_handle_table_memory_efficiency() {
    let table = HandleTable::new();

    // Verify initial memory layout is efficient
    assert_eq!(table.free_handles_len(), HandleTable::MAX_HANDLES);
    assert_eq!(table.open_count(), 0);

    // Verify that handles are allocated in ascending order
    // (due to stack-based allocation with reverse initialization)
    let temp_table = HandleTable::new();
    let mut allocated_handles = Vec::new();

    for _ in 0..10 {
        let mock_file = Arc::new(MockFileObject::new(b"test".to_vec()));
        let kernel_obj = KernelObject::File(mock_file);
        let handle = temp_table.insert(kernel_obj).unwrap();
        allocated_handles.push(handle);
    }

    // Handles should be allocated in ascending order
    for i in 0..10 {
        assert_eq!(allocated_handles[i], i as Handle);
    }
}

#[test_case]
fn test_handle_table_active_handles_accuracy() {
    let table = HandleTable::new();
    let mut expected_active = Vec::new();

    // Insert handles in non-sequential pattern
    for i in [5, 2, 8, 1, 9, 3] {
        let mock_file = Arc::new(MockFileObject::new(format!("test_{}", i).into_bytes()));
        let kernel_obj = KernelObject::File(mock_file);
        let handle = table.insert(kernel_obj).unwrap();
        expected_active.push(handle);
    }

    let mut active_handles = table.active_handles();
    active_handles.sort();
    expected_active.sort();

    assert_eq!(active_handles, expected_active);

    // Remove some handles and verify active list updates
    table.remove(expected_active[1]);
    table.remove(expected_active[3]);

    let active_after_removal = table.active_handles();
    assert_eq!(active_after_removal.len(), 4);
    assert!(!active_after_removal.contains(&expected_active[1]));
    assert!(!active_after_removal.contains(&expected_active[3]));
}

#[test_case]
fn test_handle_table_concurrent_like_operations() {
    let table = HandleTable::new();

    // Simulate concurrent-like operations by rapidly inserting and removing
    for iteration in 0..10 {
        let mut temp_handles = Vec::new();

        // Insert several objects
        for i in 0..5 {
            let mock_file = Arc::new(MockFileObject::new(
                format!("iter{}_obj{}", iteration, i).into_bytes(),
            ));
            let kernel_obj = KernelObject::File(mock_file);
            let handle = table.insert(kernel_obj).unwrap();
            temp_handles.push(handle);
        }

        // Remove them in different order
        for &handle in temp_handles.iter().rev() {
            assert!(table.remove(handle).is_some());
        }

        // Table should be empty after each iteration
        assert_eq!(table.open_count(), 0);
    }
}

#[test_case]
fn test_handle_table_sharing() {
    // Test that clone() creates a shared copy (Arc behavior)
    let table1 = HandleTable::new();

    let mock_file = Arc::new(MockFileObject::new(b"shared_test".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let handle = table1.insert(kernel_obj).unwrap();

    // Clone creates a shared reference
    let table2 = table1.clone();

    // Both tables should see the same handle
    assert!(table1.is_valid_handle(handle));
    assert!(table2.is_valid_handle(handle));
    assert_eq!(table1.open_count(), 1);
    assert_eq!(table2.open_count(), 1);

    // Removing from one should affect the other (shared)
    table2.remove(handle);
    assert!(!table1.is_valid_handle(handle));
    assert!(!table2.is_valid_handle(handle));
    assert_eq!(table1.open_count(), 0);
    assert_eq!(table2.open_count(), 0);
}

#[test_case]
fn test_handle_table_deep_clone() {
    // Test that deep_clone() creates an independent copy
    let table1 = HandleTable::new();

    let mock_file = Arc::new(MockFileObject::new(b"deep_clone_test".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let handle = table1.insert(kernel_obj).unwrap();

    // Deep clone creates an independent copy
    let table2 = table1.deep_clone();

    // Both tables should see the same handle initially
    assert!(table1.is_valid_handle(handle));
    assert!(table2.is_valid_handle(handle));
    assert_eq!(table1.open_count(), 1);
    assert_eq!(table2.open_count(), 1);

    // Removing from one should NOT affect the other (independent)
    table2.remove(handle);
    assert!(table1.is_valid_handle(handle)); // table1 still has it
    assert!(!table2.is_valid_handle(handle)); // table2 no longer has it
    assert_eq!(table1.open_count(), 1);
    assert_eq!(table2.open_count(), 0);
}

#[cfg(feature = "network")]
#[test_case]
fn close_all_closes_named_socket_while_inflight_clone_survives() {
    use crate::network::local::LocalSocket;
    use crate::network::{
        LocalSocketAddress, NetworkManager, SocketAddress, SocketError, SocketObject,
        SocketProtocol, SocketState, SocketType,
    };

    let table = HandleTable::new();
    let path = "/handle-table-close-retained-socket";
    let socket: Arc<dyn SocketObject> = Arc::new(LocalSocket::new(
        SocketType::Stream,
        SocketProtocol::Default,
    ));
    socket
        .bind(&SocketAddress::Local(
            LocalSocketAddress::from_path(path).unwrap(),
        ))
        .unwrap();
    socket.listen(1).unwrap();

    let manager = NetworkManager::get_manager();
    manager.allocate_socket_id(Arc::clone(&socket)).unwrap();
    manager
        .register_named_socket(path, Arc::clone(&socket))
        .unwrap();
    let handle = table
        .insert(KernelObject::from_socket_object(Arc::clone(&socket)))
        .unwrap();
    let in_flight = table.get(handle).unwrap();

    table.close_all();

    assert_eq!(socket.state(), SocketState::Closed);
    assert!(matches!(
        manager.lookup_named_socket(path),
        Err(SocketError::ConnectionRefused)
    ));
    assert_eq!(table.open_count(), 0);
    drop(in_flight);
}

#[cfg(feature = "network")]
#[test_case]
fn duplicated_socket_reference_defers_final_listener_close() {
    use crate::network::local::LocalSocket;
    use crate::network::{
        LocalSocketAddress, NetworkManager, SocketAddress, SocketObject, SocketProtocol,
        SocketState, SocketType,
    };

    let table = HandleTable::new();
    let path = "/duplicated-socket-defers-close";
    let socket: Arc<dyn SocketObject> = Arc::new(LocalSocket::new(
        SocketType::Stream,
        SocketProtocol::Default,
    ));
    socket
        .bind(&SocketAddress::Local(
            LocalSocketAddress::from_path(path).unwrap(),
        ))
        .unwrap();
    socket.listen(1).unwrap();

    let manager = NetworkManager::get_manager();
    manager.allocate_socket_id(Arc::clone(&socket)).unwrap();
    manager
        .register_named_socket(path, Arc::clone(&socket))
        .unwrap();
    let handle = table
        .insert(KernelObject::from_socket_object(Arc::clone(&socket)))
        .unwrap();
    let (duplicate, _) = table.clone_for_dup(handle).unwrap();

    table.close_all();
    assert_eq!(socket.state(), SocketState::Listening);
    assert!(Arc::ptr_eq(
        &manager.lookup_named_socket(path).unwrap(),
        &socket
    ));

    drop(duplicate);
    assert_eq!(socket.state(), SocketState::Closed);
    assert!(manager.lookup_named_socket(path).is_err());
}
