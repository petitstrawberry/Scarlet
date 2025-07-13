use super::super::*;
use super::mock::{MockTaskFileObject};
use crate::object::handle::HandleTable;
use crate::task::{new_user_task, CloneFlags};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::ToString;
use alloc::format;
use crate::fs::{FileType, SeekFrom};

/// Task integration tests for HandleTable
/// 
/// These tests verify that the handle table integrates correctly with the Task
/// system, including process lifecycle management, FD compatibility, cloning,
/// and error handling.

#[test_case]
fn test_task_handle_table_integration() {
    let mut task = new_user_task("TestTask".to_string(), 1);
    task.init();

    // Create some mock file objects
    let mock_file1 = Arc::new(MockTaskFileObject::new(b"test file 1".to_vec()));
    let mock_file2 = Arc::new(MockTaskFileObject::new(b"test file 2".to_vec()));

    let kernel_obj1 = KernelObject::File(mock_file1);
    let kernel_obj2 = KernelObject::File(mock_file2);

    // Insert objects into task's handle table
    let handle1 = task.handle_table.insert(kernel_obj1).unwrap();
    let handle2 = task.handle_table.insert(kernel_obj2).unwrap();

    assert_eq!(task.handle_table.open_count(), 2);
    assert_eq!(handle1, 0); // First handle should be 0
    assert_eq!(handle2, 1); // Second handle should be 1

    // Test accessing files through handles
    let retrieved_obj1 = task.handle_table.get(handle1).unwrap();
    let retrieved_obj2 = task.handle_table.get(handle2).unwrap();

    assert!(retrieved_obj1.as_stream().is_some());
    assert!(retrieved_obj2.as_stream().is_some());

    // Test reading from files
    if let Some(stream1) = retrieved_obj1.as_stream() {
        let mut buffer = [0u8; 11];
        let bytes_read = stream1.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 11);
        assert_eq!(&buffer, b"test file 1");
    }

    if let Some(stream2) = retrieved_obj2.as_stream() {
        let mut buffer = [0u8; 11];
        let bytes_read = stream2.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 11);
        assert_eq!(&buffer, b"test file 2");
    }

    // Clean up
    task.handle_table.close_all();
    assert_eq!(task.handle_table.open_count(), 0);
}

#[test_case]
fn test_task_handle_table_fd_compatibility() {
    let mut task = new_user_task("FDCompatTask".to_string(), 1);
    task.init();

    // Test that handles work like traditional file descriptors
    let mut handles = Vec::new();
    
    // Allocate several handles
    for i in 0..10 {
        let mock_file = Arc::new(MockTaskFileObject::new(
            format!("file_{}", i).into_bytes()
        ));
        let kernel_obj = KernelObject::File(mock_file);
        let handle = task.handle_table.insert(kernel_obj).unwrap();
        handles.push(handle);
    }

    // Handles should be allocated sequentially like FDs
    for (i, &handle) in handles.iter().enumerate() {
        assert_eq!(handle, i as Handle);
    }

    // Close some handles (like closing FDs)
    task.handle_table.remove(handles[3]).unwrap();
    task.handle_table.remove(handles[7]).unwrap();

    // Next allocation should reuse freed handles
    let mock_file = Arc::new(MockTaskFileObject::new(b"reused_file".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let reused_handle = task.handle_table.insert(kernel_obj).unwrap();

    // Should reuse either handle 3 or 7 (stack-based allocation)
    assert!(reused_handle == 3 || reused_handle == 7);
}

#[test_case]
fn test_task_handle_table_process_lifecycle() {
    let mut task = new_user_task("LifecycleTask".to_string(), 1);
    task.init();

    // Simulate a process opening multiple files
    let mut open_handles = Vec::new();
    
    for i in 0..5 {
        let mock_file = Arc::new(MockTaskFileObject::new(
            format!("process_file_{}", i).into_bytes()
        ));
        let kernel_obj = KernelObject::File(mock_file);
        let handle = task.handle_table.insert(kernel_obj).unwrap();
        open_handles.push(handle);
    }

    assert_eq!(task.handle_table.open_count(), 5);

    // Simulate process termination - all handles should be closed
    task.handle_table.close_all();

    assert_eq!(task.handle_table.open_count(), 0);
    assert_eq!(task.handle_table.active_handles().len(), 0);

    // All handles should now be available for reuse
    assert_eq!(task.handle_table.free_handles.len(), HandleTable::MAX_HANDLES);
}

#[test_case]
fn test_task_handle_table_error_conditions() {
    let mut task = new_user_task("ErrorTask".to_string(), 1);
    task.init();

    // Test invalid handle operations
    assert!(task.handle_table.get(999).is_none());
    assert!(!task.handle_table.is_valid_handle(999));
    assert!(task.handle_table.remove(999).is_none());

    // Test handle limit enforcement
    let mut handles = Vec::new();
    
    // Fill up to the limit
    for i in 0..HandleTable::MAX_HANDLES {
        let mock_file = Arc::new(MockTaskFileObject::new(
            format!("limit_test_{}", i).into_bytes()
        ));
        let kernel_obj = KernelObject::File(mock_file);
        let handle = task.handle_table.insert(kernel_obj).unwrap();
        handles.push(handle);
    }

    // Next insertion should fail
    let mock_file = Arc::new(MockTaskFileObject::new(b"overflow".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let result = task.handle_table.insert(kernel_obj);
    
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Too many open KernelObjects, limit reached");
}

#[test_case]
fn test_task_handle_table_clone_behavior() {
    // Test how handle table behaves during task cloning
    let mut parent_task = new_user_task("ParentTask".to_string(), 1);
    parent_task.init();

    // Open some files in parent
    let mock_file1 = Arc::new(MockTaskFileObject::new(b"parent_file_1".to_vec()));
    let mock_file2 = Arc::new(MockTaskFileObject::new(b"parent_file_2".to_vec()));

    let kernel_obj1 = KernelObject::File(mock_file1);
    let kernel_obj2 = KernelObject::File(mock_file2);

    let handle1 = parent_task.handle_table.insert(kernel_obj1).unwrap();
    let handle2 = parent_task.handle_table.insert(kernel_obj2).unwrap();

    assert_eq!(parent_task.handle_table.open_count(), 2);

    // Clone the task (this should clone the handle table)
    let mut child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

    // Child should inherit parent's handle table (Linux fork() behavior)
    assert_eq!(child_task.handle_table.open_count(), 2);
    
    // Parent's handle table should be unaffected
    assert_eq!(parent_task.handle_table.open_count(), 2);
    assert!(parent_task.handle_table.is_valid_handle(handle1));
    assert!(parent_task.handle_table.is_valid_handle(handle2));

    // Child should have inherited the same handles
    assert!(child_task.handle_table.is_valid_handle(handle1));
    assert!(child_task.handle_table.is_valid_handle(handle2));

    // Verify that child and parent have independent handle tables (closing in one doesn't affect the other)
    child_task.handle_table.remove(handle1);
    assert_eq!(child_task.handle_table.open_count(), 1);
    assert_eq!(parent_task.handle_table.open_count(), 2); // Parent still has both handles
    assert!(parent_task.handle_table.is_valid_handle(handle1)); // Parent's handle1 still valid

    // Child and parent should have independent handle tables for new allocations
    let mock_child_file = Arc::new(MockTaskFileObject::new(b"child_file".to_vec()));
    let child_kernel_obj = KernelObject::File(mock_child_file);
    let child_handle = child_task.handle_table.insert(child_kernel_obj).unwrap();

    assert_eq!(child_task.handle_table.open_count(), 2); // handle2 + new child_handle
    assert_eq!(parent_task.handle_table.open_count(), 2); // Still has both original handles
    
    // New child handle should reuse the freed handle1 slot
    assert_eq!(child_handle, handle1); // Should reuse handle1 (0)

    // Verify that the file objects are still accessible from both tasks
    // and contain the same data (Arc sharing), but positions are also shared
    if let Some(parent_obj) = parent_task.handle_table.get(handle2) {
        if let Some(child_obj) = child_task.handle_table.get(handle2) {
            if let (Some(parent_stream), Some(child_stream)) = (parent_obj.as_stream(), child_obj.as_stream()) {
                // Read from parent first - this will advance the shared position
                let mut parent_buffer = [0u8; 13];
                let parent_bytes = parent_stream.read(&mut parent_buffer).unwrap();
                assert_eq!(parent_bytes, 13);
                assert_eq!(&parent_buffer, b"parent_file_2");
                
                // Now try to read from child - position should have advanced, so it returns 0
                let mut child_buffer = [0u8; 13];
                let child_bytes = child_stream.read(&mut child_buffer).unwrap();
                assert_eq!(child_bytes, 0); // No more data to read because position is at EOF
                
                // Reset position using parent's file object and try again
                if let Some(parent_file) = parent_obj.as_file() {
                    parent_file.seek(SeekFrom::Start(0)).unwrap(); // Reset to beginning
                    
                    // Now child should be able to read from the beginning
                    let child_bytes = child_stream.read(&mut child_buffer).unwrap();
                    assert_eq!(child_bytes, 13);
                    assert_eq!(&child_buffer, b"parent_file_2");
                }
            }
        }
    }
}

#[test_case]
fn test_task_handle_table_memory_efficiency() {
    let mut task = new_user_task("MemoryTask".to_string(), 1);
    task.init();

    // Test that repeated allocation/deallocation doesn't cause memory leaks
    for iteration in 0..50 {
        let mut temp_handles = Vec::new();

        // Allocate some handles
        for i in 0..20 {
            let mock_file = Arc::new(MockTaskFileObject::new(
                format!("iter_{}_file_{}", iteration, i).into_bytes()
            ));
            let kernel_obj = KernelObject::File(mock_file);
            let handle = task.handle_table.insert(kernel_obj).unwrap();
            temp_handles.push(handle);
        }

        assert_eq!(task.handle_table.open_count(), 20);

        // Free all handles
        for handle in temp_handles {
            assert!(task.handle_table.remove(handle).is_some());
        }

        assert_eq!(task.handle_table.open_count(), 0);
    }

    // After all iterations, handle table should be in clean state
    assert_eq!(task.handle_table.open_count(), 0);
    assert_eq!(task.handle_table.free_handles.len(), HandleTable::MAX_HANDLES);
}

#[test_case]
fn test_task_handle_table_capability_access() {
    let mut task = new_user_task("CapabilityTask".to_string(), 1);
    task.init();

    // Test accessing different capabilities through handles
    let mock_file = Arc::new(MockTaskFileObject::new(b"capability_test_data".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    let handle = task.handle_table.insert(kernel_obj).unwrap();

    // Test stream capability
    if let Some(obj) = task.handle_table.get(handle) {
        if let Some(stream) = obj.as_stream() {
            let mut buffer = [0u8; 10];
            let bytes_read = stream.read(&mut buffer).unwrap();
            assert_eq!(bytes_read, 10);
            assert_eq!(&buffer, b"capability");

            let bytes_written = stream.write(b"test").unwrap();
            assert_eq!(bytes_written, 4);
        } else {
            panic!("Expected stream capability");
        }

        // Test file capability
        if let Some(file) = obj.as_file() {
            let metadata = file.metadata().unwrap();
            assert_eq!(metadata.file_type, FileType::RegularFile);
            assert_eq!(metadata.size, 20); // "capability_test_data".len()

            let position = file.seek(SeekFrom::Start(5)).unwrap();
            assert_eq!(position, 5);
        } else {
            panic!("Expected file capability");
        }
    } else {
        panic!("Failed to get object from handle");
    }
}
