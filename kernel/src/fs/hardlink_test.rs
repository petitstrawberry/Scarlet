//! Test module for hardlink functionality
//! 
//! This module contains tests to verify that hardlink implementation
//! works correctly across different filesystem implementations.

use super::*;
use super::tmpfs::TmpFS;

#[cfg(test)]
mod tests {
    use super::*;

    /// Test basic hardlink creation and verification
    #[test_case]
    fn test_hardlink_creation() {
        // Create TmpFS instance directly (not wrapped in Arc<RwLock>)
        let tmpfs = TmpFS::new(1024 * 1024); // 1MB limit
        let manager = VfsManager::new();
        
        // Register and mount filesystem
        let fs_id = manager.register_fs(Box::new(tmpfs));
        manager.mount(fs_id, "/tmp").expect("Failed to mount tmpfs");
        
        // Create original file
        manager.create_regular_file("/tmp/original.txt").expect("Failed to create original file");
        
        // Write some data
        let mut file = manager.open("/tmp/original.txt", 0).expect("Failed to open original file");
        file.write(b"Hello, hardlink test!").expect("Failed to write to file");
        drop(file);
        
        // Create hardlink
        manager.create_hardlink("/tmp/original.txt", "/tmp/link.txt").expect("Failed to create hardlink");
        
        // Debug: Check if files actually exist and have content
        let original_metadata = manager.metadata("/tmp/original.txt").expect("Failed to get original metadata");
        let link_metadata = manager.metadata("/tmp/link.txt").expect("Failed to get link metadata");
        
        // Should have same file_id and link_count = 2
        assert_eq!(original_metadata.file_id, link_metadata.file_id);
        assert_eq!(original_metadata.link_count, 2);
        assert_eq!(link_metadata.link_count, 2);
        
        // Content should be identical
        let mut original_file = manager.open("/tmp/original.txt", 0).expect("Failed to open original");
        let mut link_file = manager.open("/tmp/link.txt", 0).expect("Failed to open link");
        
        let original_content = original_file.read_all().expect("Failed to read original");
        let link_content = link_file.read_all().expect("Failed to read link");
    
        assert_eq!(original_content, link_content);
        assert_eq!(original_content, b"Hello, hardlink test!");
    }
    
    /// Test hardlink removal behavior
    #[test_case]
    fn test_hardlink_removal() {
        let tmpfs = TmpFS::new(1024 * 1024);
        let manager = VfsManager::new();
        
        let fs_id = manager.register_fs(Box::new(tmpfs));
        manager.mount(fs_id, "/tmp").expect("Failed to mount tmpfs");
        
        // Create file and hardlink
        manager.create_regular_file("/tmp/file.txt").expect("Failed to create file");
        
        let mut file = manager.open("/tmp/file.txt", 0).expect("Failed to open file");
        file.write(b"test data").expect("Failed to write data");
        drop(file);
        
        manager.create_hardlink("/tmp/file.txt", "/tmp/link1.txt").expect("Failed to create link1");
        manager.create_hardlink("/tmp/file.txt", "/tmp/link2.txt").expect("Failed to create link2");
        
        // Verify link_count = 3
        let metadata = manager.metadata("/tmp/file.txt").expect("Failed to get metadata");
        assert_eq!(metadata.link_count, 3);
        
        // Remove one link
        manager.remove("/tmp/link1.txt").expect("Failed to remove link1");
        
        // Verify link_count = 2, other files still exist
        let metadata = manager.metadata("/tmp/file.txt").expect("Failed to get metadata after removal");
        assert_eq!(metadata.link_count, 2);
        
        // link1 should be gone
        assert!(manager.metadata("/tmp/link1.txt").is_err());
        
        // file.txt and link2.txt should still exist
        assert!(manager.metadata("/tmp/file.txt").is_ok());
        assert!(manager.metadata("/tmp/link2.txt").is_ok());
        
        // Content should still be accessible
        let mut remaining_file = manager.open("/tmp/link2.txt", 0).expect("Failed to open remaining link");
        let content = remaining_file.read_all().expect("Failed to read content");
        assert_eq!(content, b"test data");
    }
    
    /// Test that hardlink to directory fails
    #[test_case]
    fn test_hardlink_directory_fails() {
        let tmpfs = TmpFS::new(1024 * 1024);
        let manager = VfsManager::new();
        
        let fs_id = manager.register_fs(Box::new(tmpfs));
        manager.mount(fs_id, "/tmp").expect("Failed to mount tmpfs");
        
        // Create directory
        manager.create_dir("/tmp/testdir").expect("Failed to create directory");
        
        // Try to create hardlink to directory - should fail
        let result = manager.create_hardlink("/tmp/testdir", "/tmp/dirlink");
        assert!(result.is_err());
        
        if let Err(error) = result {
            assert_eq!(error.kind, FileSystemErrorKind::NotSupported);
        }
    }
    
    /// Test cross-filesystem hardlink fails
    #[test_case]
    fn test_cross_filesystem_hardlink_fails() {
        let tmpfs1 = TmpFS::new(1024 * 1024);
        let tmpfs2 = TmpFS::new(1024 * 1024);
        let manager = VfsManager::new();
        
        // Mount two different filesystems
        let fs1_id = manager.register_fs(Box::new(tmpfs1));
        let fs2_id = manager.register_fs(Box::new(tmpfs2));
        manager.mount(fs1_id, "/tmp1").expect("Failed to mount tmpfs1");
        manager.mount(fs2_id, "/tmp2").expect("Failed to mount tmpfs2");
        
        // Create file in first filesystem
        manager.create_regular_file("/tmp1/file.txt").expect("Failed to create file");
        
        // Try to create hardlink in second filesystem - should fail
        let result = manager.create_hardlink("/tmp1/file.txt", "/tmp2/link.txt");
        assert!(result.is_err());
        
        if let Err(error) = result {
            assert_eq!(error.kind, FileSystemErrorKind::NotSupported);
        }
    }
}
