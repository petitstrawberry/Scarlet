//! KernelObject tests

use super::mock::MockFileObject;
use crate::object::KernelObject;
use crate::fs::{FileType, SeekFrom};
use alloc::{sync::Arc, vec::Vec};

#[test_case]
fn test_kernel_object_creation() {
    let mock_file = Arc::new(MockFileObject::new(b"test data".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    
    // Test as_stream capability
    assert!(kernel_obj.as_stream().is_some());
    
    // Test as_file capability
    assert!(kernel_obj.as_file().is_some());
}

#[test_case]
fn test_kernel_object_stream_operations() {
    let mock_file = Arc::new(MockFileObject::new(b"Hello, World!".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    
    if let Some(stream) = kernel_obj.as_stream() {
        // Test read operation
        let mut buffer = [0u8; 5];
        let bytes_read = stream.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 5);
        assert_eq!(&buffer, b"Hello");
        
        // Test write operation
        let bytes_written = stream.write(b"test").unwrap();
        assert_eq!(bytes_written, 4);
    } else {
        panic!("Expected stream capability");
    }
}

#[test_case]
fn test_kernel_object_file_operations() {
    let mock_file = Arc::new(MockFileObject::new(b"test file content".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    
    if let Some(file) = kernel_obj.as_file() {
        // Test metadata
        let metadata = file.metadata().unwrap();
        assert_eq!(metadata.size, 17);
        assert_eq!(metadata.file_type, FileType::RegularFile);
        
        // Test seek
        let position = file.seek(SeekFrom::Start(5)).unwrap();
        assert_eq!(position, 5);
        
        // Test readdir (should fail for regular file)
        assert!(file.readdir().is_err());
    } else {
        panic!("Expected file capability");
    }
}

#[test_case]
fn test_kernel_object_capabilities_consistency() {
    let mock_file = Arc::new(MockFileObject::new(b"capability test".to_vec()));
    let kernel_obj = KernelObject::File(mock_file.clone());
    
    // Test that as_stream and as_file return consistent capabilities
    let stream = kernel_obj.as_stream().unwrap();
    let file = kernel_obj.as_file().unwrap();
    
    // Both should be backed by the same object
    let mut buffer1 = [0u8; 4];
    let mut buffer2 = [0u8; 4];
    
    // Read from stream
    let read1 = stream.read(&mut buffer1).unwrap();
    assert_eq!(read1, 4);
    assert_eq!(&buffer1, b"capa");
    
    // The file should have the same position state
    let read2 = file.read(&mut buffer2).unwrap();
    assert_eq!(read2, 4);
    assert_eq!(&buffer2, b"bili");
}

#[test_case]
fn test_kernel_object_error_propagation() {
    // Create a file object that will cause seek errors
    let mock_file = Arc::new(MockFileObject::new(Vec::new())); // Empty file
    let kernel_obj = KernelObject::File(mock_file);
    
    if let Some(file) = kernel_obj.as_file() {
        // Test that metadata works even for empty files
        let metadata = file.metadata().unwrap();
        assert_eq!(metadata.size, 0);
        
        // Test seek operations
        assert!(file.seek(SeekFrom::Start(0)).is_ok());
        
        // Readdir should fail for regular files
        assert!(file.readdir().is_err());
    }
}

#[test_case]
fn test_kernel_object_drop_behavior() {
    // Test that KernelObject properly calls release() when dropped
    let mock_file = Arc::new(MockFileObject::new(b"test drop".to_vec()));
    let kernel_obj = KernelObject::File(mock_file);
    
    // Object should automatically call release() when dropped
    drop(kernel_obj);
    // This test mainly checks that no panic occurs during drop
}

#[test_case]
fn test_kernel_object_clone_behavior() {
    let mock_file = Arc::new(MockFileObject::new(b"clone test".to_vec()));
    let kernel_obj1 = KernelObject::File(mock_file);
    
    // Clone the kernel object
    let kernel_obj2 = kernel_obj1.clone();
    
    // Both should work independently but share the same underlying file
    assert!(kernel_obj1.as_stream().is_some());
    assert!(kernel_obj2.as_stream().is_some());
    assert!(kernel_obj1.as_file().is_some());
    assert!(kernel_obj2.as_file().is_some());
    
    // Test that operations on one affect the other (shared state)
    if let (Some(stream1), Some(stream2)) = (kernel_obj1.as_stream(), kernel_obj2.as_stream()) {
        let mut buffer1 = [0u8; 5];
        let mut buffer2 = [0u8; 5];
        
        // Read from first stream
        let read1 = stream1.read(&mut buffer1).unwrap();
        assert_eq!(read1, 5);
        assert_eq!(&buffer1, b"clone");
        
        // Read from second stream should continue from where first left off
        let read2 = stream2.read(&mut buffer2).unwrap();
        assert_eq!(read2, 5);
        assert_eq!(&buffer2, b" test");
    }
}
