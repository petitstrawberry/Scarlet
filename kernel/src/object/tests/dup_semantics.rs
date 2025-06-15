//! Tests to demonstrate the dup semantics problem discussed in the exploration

use alloc::sync::Arc;
use crate::object::KernelObject;
use crate::ipc::pipe::UnidirectionalPipe;
use crate::ipc::IpcObject;
use super::mock::MockFileObject;

#[test_case]
fn test_kernelobj_clone_file_semantics() {
    // Files should share position through Arc cloning
    let mock_file = Arc::new(MockFileObject::new(b"Hello World".to_vec()));
    let kernel_obj1 = KernelObject::File(mock_file);
    
    // Clone via KernelObject::clone (simulates dup())
    let kernel_obj2 = kernel_obj1.clone();
    
    // Both should share file position
    if let (Some(stream1), Some(stream2)) = (kernel_obj1.as_stream(), kernel_obj2.as_stream()) {
        let mut buffer1 = [0u8; 5];
        let mut buffer2 = [0u8; 6];
        
        // Read from first instance
        let read1 = stream1.read(&mut buffer1).unwrap();
        assert_eq!(read1, 5);
        assert_eq!(&buffer1, b"Hello");
        
        // Read from second instance should continue from where first left off
        let read2 = stream2.read(&mut buffer2).unwrap();
        assert_eq!(read2, 6);
        assert_eq!(&buffer2, b" World");
    } else {
        panic!("Expected stream capability");
    }
}

#[test_case]
fn test_kernelobj_clone_pipe_fixed() {
    // Create a pipe pair
    let (read_obj, write_obj) = UnidirectionalPipe::create_pair(1024);
    
    // Check initial state
    if let Some(read_pipe) = read_obj.as_pipe() {
        if let Some(write_pipe) = write_obj.as_pipe() {
            // Initially: 1 reader, 1 writer
            assert_eq!(read_pipe.peer_count(), 1); // 1 writer
            assert_eq!(write_pipe.peer_count(), 1); // 1 reader
            assert!(read_pipe.has_writers());
            assert!(write_pipe.has_readers());
        }
    }
    
    // Clone the read end via KernelObject::clone (simulates dup())
    let read_obj_cloned = read_obj.clone();
    
    // FIXED: This should increment reader count, and now it does!
    if let Some(read_pipe) = read_obj.as_pipe() {
        if let Some(write_pipe) = write_obj.as_pipe() {
            if let Some(read_pipe_cloned) = read_obj_cloned.as_pipe() {
                // EXPECTED: write_pipe.peer_count() should be 2 (two readers)
                // ACTUAL: write_pipe.peer_count() is now correctly 2!
                // This demonstrates the fix is working!
                assert_eq!(write_pipe.peer_count(), 2); // Two readers now!
                
                // Both read endpoints should have the same writer count
                assert_eq!(read_pipe.peer_count(), 1);
                assert_eq!(read_pipe_cloned.peer_count(), 1);
            }
        }
    }
}

#[test_case]
fn test_direct_pipe_clone_works() {
    // Test that direct cloning of pipe endpoints works correctly
    let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(1024);
    
    // Initially: 1 reader, 1 writer
    assert_eq!(read_end.peer_count(), 1); // 1 writer
    assert_eq!(write_end.peer_count(), 1); // 1 reader
    
    // Clone the read end directly (not through KernelObject)
    let read_end_cloned = read_end.clone();
    
    // Now we should see 2 readers
    assert_eq!(read_end.peer_count(), 1); // Still 1 writer
    assert_eq!(read_end_cloned.peer_count(), 1); // Still 1 writer
    assert_eq!(write_end.peer_count(), 2); // NOW 2 readers!
    
    // This demonstrates that the custom Clone implementation works
    // when called directly, but not through KernelObject::clone()
}
