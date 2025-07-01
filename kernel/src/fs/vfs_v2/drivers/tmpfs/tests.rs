#[cfg(test)]
mod tests {
    use crate::fs::drivers::tmpfs::TmpFS;
    use crate::fs::vfs_v2::manager::VfsManager;
    use crate::fs::{FileType, FileSystemErrorKind};
    use alloc::sync::Arc;

    /// Test basic hard link creation and functionality
    #[test_case]
    fn test_hardlink_basic() {
        // Create TmpFS and VFS manager
        let tmpfs = TmpFS::new(0); // Unlimited memory
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create a test file
        vfs.create_file("/testfile.txt", FileType::RegularFile).unwrap();
        
        // Write some content to the file
        let file = vfs.open("/testfile.txt", 0x02).unwrap(); // Write mode
        if let crate::object::KernelObject::File(file_obj) = file {
            file_obj.write(b"Hello, hardlink test!").unwrap();
        }

        // Create a hard link
        vfs.create_hardlink("/testfile.txt", "/hardlink.txt").unwrap();

        // Verify both files exist and have the same content
        let original = vfs.open("/testfile.txt", 0x01).unwrap(); // Read mode
        let hardlink = vfs.open("/hardlink.txt", 0x01).unwrap(); // Read mode

        if let (crate::object::KernelObject::File(orig_obj), crate::object::KernelObject::File(link_obj)) = 
            (original, hardlink) {
            let mut orig_buf = [0u8; 64];
            let mut link_buf = [0u8; 64];
            
            let orig_len = orig_obj.read(&mut orig_buf).unwrap();
            let link_len = link_obj.read(&mut link_buf).unwrap();
            
            assert_eq!(orig_len, link_len);
            assert_eq!(&orig_buf[..orig_len], &link_buf[..link_len]);
        }
    }

    /// Test that modifying content through one hardlink affects the other
    #[test_case]
    fn test_hardlink_shared_content() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create file and hardlink
        vfs.create_file("/original.txt", FileType::RegularFile).unwrap();
        vfs.create_hardlink("/original.txt", "/link.txt").unwrap();

        // Write through the hardlink
        let link_file = vfs.open("/link.txt", 0x02).unwrap(); // Write mode
        if let crate::object::KernelObject::File(file_obj) = link_file {
            file_obj.write(b"Modified through hardlink").unwrap();
        }

        // Read through the original
        let orig_file = vfs.open("/original.txt", 0x01).unwrap(); // Read mode
        if let crate::object::KernelObject::File(file_obj) = orig_file {
            let mut buf = [0u8; 64];
            let len = file_obj.read(&mut buf).unwrap();
            let content = core::str::from_utf8(&buf[..len]).unwrap();
            assert_eq!(content, "Modified through hardlink");
        }
    }

    /// Test hardlink link count metadata
    #[test_case]
    fn test_hardlink_link_count() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create a file
        vfs.create_file("/file.txt", FileType::RegularFile).unwrap();
        
        // Get initial metadata
        let (entry, _) = vfs.mount_tree.resolve_path("/file.txt").unwrap();
        let initial_metadata = entry.node().metadata().unwrap();
        assert_eq!(initial_metadata.link_count, 1);

        // Create hardlink
        vfs.create_hardlink("/file.txt", "/link1.txt").unwrap();
        
        // Check link count increased
        let updated_metadata = entry.node().metadata().unwrap();
        assert_eq!(updated_metadata.link_count, 2);

        // Create another hardlink
        vfs.create_hardlink("/file.txt", "/link2.txt").unwrap();
        
        // Check link count increased again
        let final_metadata = entry.node().metadata().unwrap();
        assert_eq!(final_metadata.link_count, 3);
    }

    /// Test hardlink error conditions
    #[test_case]
    fn test_hardlink_errors() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Test linking to non-existent file
        let result = vfs.create_hardlink("/nonexistent.txt", "/link.txt");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::NotFound));
        }

        // Create a directory and try to hardlink it
        vfs.create_file("/testdir", FileType::Directory).unwrap();
        let result = vfs.create_hardlink("/testdir", "/dirlink");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::InvalidOperation));
        }

        // Create a file and try to link to existing name
        vfs.create_file("/original.txt", FileType::RegularFile).unwrap();
        vfs.create_file("/existing.txt", FileType::RegularFile).unwrap();
        let result = vfs.create_hardlink("/original.txt", "/existing.txt");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::FileExists));
        }
    }

    /// Test hardlinks in subdirectories
    #[test_case]
    fn test_hardlink_subdirectories() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create directory structure
        vfs.create_file("/subdir", FileType::Directory).unwrap();
        vfs.create_file("/subdir/file.txt", FileType::RegularFile).unwrap();
        vfs.create_file("/another", FileType::Directory).unwrap();

        // Write some content
        let file = vfs.open("/subdir/file.txt", 0x02).unwrap();
        if let crate::object::KernelObject::File(file_obj) = file {
            file_obj.write(b"Subdirectory file").unwrap();
        }

        // Create hardlink in different directory
        vfs.create_hardlink("/subdir/file.txt", "/another/hardlink.txt").unwrap();

        // Verify content is accessible from both paths
        let orig = vfs.open("/subdir/file.txt", 0x01).unwrap();
        let link = vfs.open("/another/hardlink.txt", 0x01).unwrap();

        if let (crate::object::KernelObject::File(orig_obj), crate::object::KernelObject::File(link_obj)) = 
            (orig, link) {
            let mut orig_buf = [0u8; 32];
            let mut link_buf = [0u8; 32];
            
            let orig_len = orig_obj.read(&mut orig_buf).unwrap();
            let link_len = link_obj.read(&mut link_buf).unwrap();
            
            assert_eq!(orig_len, link_len);
            assert_eq!(&orig_buf[..orig_len], b"Subdirectory file");
            assert_eq!(&orig_buf[..orig_len], &link_buf[..link_len]);
        }
    }

    /// Test that hardlinks share the same file ID
    #[test_case]
    fn test_hardlink_same_file_id() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create file and hardlink
        vfs.create_file("/original.txt", FileType::RegularFile).unwrap();
        vfs.create_hardlink("/original.txt", "/hardlink.txt").unwrap();

        // Get metadata for both
        let (orig_entry, _) = vfs.mount_tree.resolve_path("/original.txt").unwrap();
        let (link_entry, _) = vfs.mount_tree.resolve_path("/hardlink.txt").unwrap();

        let orig_metadata = orig_entry.node().metadata().unwrap();
        let link_metadata = link_entry.node().metadata().unwrap();

        // Should have same file ID (same underlying file)
        assert_eq!(orig_metadata.file_id, link_metadata.file_id);
        
        // Should have same size
        assert_eq!(orig_metadata.size, link_metadata.size);
        
        // Both should show link count of 2
        assert_eq!(orig_metadata.link_count, 2);
        assert_eq!(link_metadata.link_count, 2);
    }
}
