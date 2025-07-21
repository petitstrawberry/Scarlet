#[cfg(test)]
mod tests {
    use crate::fs::drivers::tmpfs::TmpFS;
    use crate::fs::vfs_v2::manager::VfsManager;
    use crate::fs::{FileType, FileSystemErrorKind};
    use alloc::string::ToString;

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

    // ===== SYMBOLIC LINK TESTS =====

    /// Test basic symbolic link creation and target reading
    #[test_case]
    fn test_symlink_basic() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create a target file
        vfs.create_file("/target.txt", FileType::RegularFile).unwrap();
        
        // Create symbolic link
        vfs.create_symlink("/symlink.txt", "/target.txt").unwrap();
        
        // Use no_follow option to get the symlink itself, not the target
        let (symlink_entry, _) = vfs.resolve_path_with_options("/symlink.txt", &PathResolutionOptions::no_follow()).unwrap();
        let symlink_node = symlink_entry.node();
        
        // Debug output
        let metadata = symlink_node.metadata().unwrap();
        crate::println!("Debug: symlink metadata: {:?}", metadata);
        let file_type = symlink_node.file_type().unwrap();
        crate::println!("Debug: symlink file_type: {:?}", file_type);
        
        // Verify it's a symbolic link
        assert!(symlink_node.is_symlink().unwrap());
        assert!(matches!(symlink_node.file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Read the target
        let target = symlink_node.read_link().unwrap();
        assert_eq!(target, "/target.txt");
        
        // Also test that normal resolution follows the symlink
        let (target_entry, _) = vfs.resolve_path("/symlink.txt").unwrap();
        let target_node = target_entry.node();
        assert_eq!(target_node.file_type().unwrap(), FileType::RegularFile);
    }

    /// Test symlink with relative path
    #[test_case]
    fn test_symlink_relative_path() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create directory and file
        vfs.create_file("/subdir", FileType::Directory).unwrap();
        vfs.create_file("/subdir/target.txt", FileType::RegularFile).unwrap();
        
        // Create symbolic link with relative path
        vfs.create_symlink("/subdir/link_to_target.txt", "target.txt").unwrap();
        
        // Use no_follow to get the symlink itself
        let (symlink_entry, _) = vfs.resolve_path_with_options("/subdir/link_to_target.txt", &PathResolutionOptions::no_follow()).unwrap();
        let symlink_node = symlink_entry.node();
        
        // Verify it's a symbolic link
        assert!(symlink_node.is_symlink().unwrap());
        
        // Read the target
        let target = symlink_node.read_link().unwrap();
        assert_eq!(target, "target.txt");
        
        // Test that normal resolution follows the symlink
        let (target_entry, _) = vfs.resolve_path("/subdir/link_to_target.txt").unwrap();
        let target_node = target_entry.node();
        assert_eq!(target_node.file_type().unwrap(), FileType::RegularFile);
    }

    /// Test symlink metadata
    #[test_case]
    fn test_symlink_metadata() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        let target_path = "/some/long/target/path.txt".to_string();
        
        // Create symbolic link
        vfs.create_symlink("/symlink.txt", &target_path).unwrap();
        
        // Use no_follow to get the symlink itself
        let (symlink_entry, _) = vfs.resolve_path_with_options("/symlink.txt", &PathResolutionOptions::no_follow()).unwrap();
        let symlink_node = symlink_entry.node();
        let metadata = symlink_node.metadata().unwrap();

        // Check metadata properties
        assert!(matches!(metadata.file_type, FileType::SymbolicLink(_)));
        assert_eq!(metadata.size, target_path.len()); // Size should be target path length
        assert_eq!(metadata.link_count, 1);
        assert!(metadata.permissions.read);
        assert!(metadata.permissions.write);
        assert!(!metadata.permissions.execute);
    }

    /// Test symlink error conditions
    #[test_case]
    fn test_symlink_errors() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Test creating symlink with existing name
        vfs.create_file("/existing.txt", FileType::RegularFile).unwrap();
        let result = vfs.create_symlink("/existing.txt", "/target.txt");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::AlreadyExists));
        }

        // Test creating symlink in non-directory (this should fail at path resolution level)
        vfs.create_file("/file.txt", FileType::RegularFile).unwrap();
        let result = vfs.create_symlink("/file.txt/symlink.txt", "/target.txt");
        assert!(result.is_err());
        // This will fail because "/file.txt" is not a directory, so we can't resolve "/file.txt/symlink.txt"
    }

    /// Test reading link from non-symlink returns error
    #[test_case]
    fn test_read_link_error() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create regular file
        vfs.create_file("/regular.txt", FileType::RegularFile).unwrap();
        let (file_entry, _) = vfs.mount_tree.resolve_path("/regular.txt").unwrap();
        
        // Try to read link from regular file
        let result = file_entry.node().read_link();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::NotSupported));
        }

        // Create directory
        vfs.create_file("/dir", FileType::Directory).unwrap();
        let (dir_entry, _) = vfs.mount_tree.resolve_path("/dir").unwrap();
        
        // Try to read link from directory
        let result = dir_entry.node().read_link();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::NotSupported));
        }
    }

    /// Test symlink removal and memory cleanup
    #[test_case]
    fn test_symlink_removal() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(1024); // Limited memory to test cleanup
        let vfs = VfsManager::new_with_root(tmpfs);

        let target_path = "/very/long/target/path/for/memory/test.txt".to_string();
        
        // Create symbolic link
        let create_result = vfs.create_symlink("/symlink.txt", &target_path);
        if let Err(ref e) = create_result {
            crate::println!("Debug: Create symlink failed with error: {:?}", e);
        }
        create_result.unwrap();

        // Verify symlink was created
        let symlink_result = vfs.resolve_path_with_options("/symlink.txt", &PathResolutionOptions::no_follow());
        if let Err(ref e) = symlink_result {
            crate::println!("Debug: Resolve symlink failed with error: {:?}", e);
            // Try to list root directory to see what's there
            match vfs.readdir("/") {
                Ok(entries) => {
                    crate::println!("Debug: Root directory contents:");
                    for entry in entries {
                        crate::println!("  - {}", entry.name);
                    }
                }
                Err(e) => crate::println!("Debug: Failed to read root directory: {:?}", e),
            }
        }
        assert!(symlink_result.is_ok(), "Symlink should exist after creation");

        // Remove the symlink
        let remove_result = vfs.remove("/symlink.txt");
        if let Err(ref e) = remove_result {
            crate::println!("Debug: Remove failed with error: {:?}", e);
        }
        remove_result.unwrap();

        // Verify symlink is gone
        let result = vfs.resolve_path("/symlink.txt");
        assert!(result.is_err());
    }

    /// Test symlinks in subdirectories
    #[test_case]
    fn test_symlink_subdirectories() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create directory structure
        vfs.create_file("/dir1", FileType::Directory).unwrap();
        vfs.create_file("/dir2", FileType::Directory).unwrap();
        vfs.create_file("/dir1/target.txt", FileType::RegularFile).unwrap();

        // Create symlink in different directory
        vfs.create_symlink("/dir2/link_to_target.txt", "/dir1/target.txt").unwrap();
        
        // Use no_follow to get the symlink itself, not the target
        let (symlink_entry, _) = vfs.resolve_path_with_options("/dir2/link_to_target.txt", &PathResolutionOptions::no_follow()).unwrap();
        let symlink_node = symlink_entry.node();
        
        // Verify it's a symlink and get target
        assert!(symlink_node.is_symlink().unwrap());
        let target = symlink_node.read_link().unwrap();
        assert_eq!(target, "/dir1/target.txt");
    }

    /// Test symlink creation directly in TmpFS (not through VFS)
    #[test_case]
    fn test_symlink_direct_tmpfs() {
        use crate::fs::vfs_v2::core::FileSystemOperations;
        
        let tmpfs = TmpFS::new(0);
        let root_node = tmpfs.root_node();
        
        // Create symbolic link directly through TmpFS
        let symlink_node = tmpfs.create(
            &root_node,
            &"symlink.txt".to_string(),
            FileType::SymbolicLink("/target.txt".to_string()),
            0o644,
        ).unwrap();
        
        // Debug output
        let metadata = symlink_node.metadata().unwrap();
        crate::println!("Debug: direct tmpfs symlink metadata: {:?}", metadata);
        let file_type = symlink_node.file_type().unwrap();
        crate::println!("Debug: direct tmpfs symlink file_type: {:?}", file_type);
        
        // Verify it's a symbolic link
        assert!(symlink_node.is_symlink().unwrap());
        assert!(matches!(symlink_node.file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Read the target
        let target = symlink_node.read_link().unwrap();
        assert_eq!(target, "/target.txt");
    }

    /// Test removing files through symlink directories (ensuring intermediate symlinks are followed)
    #[test_case] 
    fn test_remove_through_symlink_directory() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create directory structure: /real_dir/file.txt
        vfs.create_dir("/real_dir").unwrap();
        vfs.create_file("/real_dir/file.txt", FileType::RegularFile).unwrap();
        
        // Create symlink to directory: /symlink_dir -> /real_dir
        vfs.create_symlink("/symlink_dir", "/real_dir").unwrap();
        
        // Verify we can access file through symlink directory
        let (file_through_symlink, _) = vfs.resolve_path("/symlink_dir/file.txt").unwrap();
        assert_eq!(file_through_symlink.node().file_type().unwrap(), FileType::RegularFile);
        
        // Remove file through symlink directory path
        // This should follow the intermediate symlink (/symlink_dir) but remove the actual file
        vfs.remove("/symlink_dir/file.txt").unwrap();
        
        // Verify file is removed from real directory
        let result = vfs.resolve_path("/real_dir/file.txt");
        assert!(result.is_err(), "File should be removed from real directory");
        
        // Verify symlink directory still exists
        let (symlink_dir, _) = vfs.resolve_path_with_options("/symlink_dir", &PathResolutionOptions::no_follow()).unwrap();
        assert!(symlink_dir.node().is_symlink().unwrap(), "Symlink directory should still exist");
    }

    /// Test removing a symlink when it's the final component of a path through symlink directories
    #[test_case]
    fn test_remove_symlink_through_symlink_directory() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create directory structure
        vfs.create_dir("/real_dir").unwrap();
        vfs.create_file("/real_dir/target.txt", FileType::RegularFile).unwrap();
        
        // Create symlink to directory: /symlink_dir -> /real_dir  
        vfs.create_symlink("/symlink_dir", "/real_dir").unwrap();
        
        // Create symlink inside the real directory: /real_dir/link_to_target -> target.txt
        vfs.create_symlink("/real_dir/link_to_target", "target.txt").unwrap();
        
        // Verify we can access the symlink through symlink directory
        let (symlink_through_dir, _) = vfs.resolve_path_with_options("/symlink_dir/link_to_target", &PathResolutionOptions::no_follow()).unwrap();
        assert!(symlink_through_dir.node().is_symlink().unwrap());
        
        // Remove the symlink through symlink directory path
        // This should follow /symlink_dir but remove the symlink /real_dir/link_to_target
        vfs.remove("/symlink_dir/link_to_target").unwrap();
        
        // Verify symlink is removed from real directory
        let result = vfs.resolve_path_with_options("/real_dir/link_to_target", &PathResolutionOptions::no_follow());
        assert!(result.is_err(), "Symlink should be removed from real directory");
        
        // Verify target file still exists
        let (target_file, _) = vfs.resolve_path("/real_dir/target.txt").unwrap();
        assert_eq!(target_file.node().file_type().unwrap(), FileType::RegularFile);
        
        // Verify symlink directory still exists
        let (symlink_dir, _) = vfs.resolve_path_with_options("/symlink_dir", &PathResolutionOptions::no_follow()).unwrap();
        assert!(symlink_dir.node().is_symlink().unwrap());
    }

    /// Test file creation through directory symlink
    #[test_case]
    fn test_file_creation_through_directory_symlink() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create directory structure
        vfs.create_dir("/real_dir").unwrap();
        
        // Create symlink to directory: /symlink_dir -> /real_dir
        vfs.create_symlink("/symlink_dir", "/real_dir").unwrap();
        
        // Create file through symlink directory
        vfs.create_file("/symlink_dir/new_file.txt", FileType::RegularFile).unwrap();
        
        // Verify file exists in real directory
        let (file_in_real, _) = vfs.resolve_path("/real_dir/new_file.txt").unwrap();
        assert_eq!(file_in_real.node().file_type().unwrap(), FileType::RegularFile);
        
        // Verify file is accessible through symlink
        let (file_through_symlink, _) = vfs.resolve_path("/symlink_dir/new_file.txt").unwrap();
        assert_eq!(file_through_symlink.node().file_type().unwrap(), FileType::RegularFile);
        
        // Both paths should resolve to the same node
        assert_eq!(file_in_real.node().id(), file_through_symlink.node().id());
    }

    /// Test file open/read/write through directory symlink
    #[test_case]
    fn test_file_operations_through_directory_symlink() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create directory and file
        vfs.create_dir("/real_dir").unwrap();
        vfs.create_file("/real_dir/data.txt", FileType::RegularFile).unwrap();
        
        // Create symlink to directory
        vfs.create_symlink("/symlink_dir", "/real_dir").unwrap();
        
        // Write data through symlink path
        let write_file = vfs.open("/symlink_dir/data.txt", 0x02).unwrap(); // Write mode
        if let crate::object::KernelObject::File(file_obj) = write_file {
            file_obj.write(b"Hello through symlink!").unwrap();
        }
        
        // Read data through real path
        let read_file = vfs.open("/real_dir/data.txt", 0x01).unwrap(); // Read mode
        if let crate::object::KernelObject::File(file_obj) = read_file {
            let mut buffer = [0u8; 64];
            let bytes_read = file_obj.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..bytes_read], b"Hello through symlink!");
        }
        
        // Read data through symlink path as well
        let read_symlink = vfs.open("/symlink_dir/data.txt", 0x01).unwrap();
        if let crate::object::KernelObject::File(file_obj) = read_symlink {
            let mut buffer = [0u8; 64];
            let bytes_read = file_obj.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..bytes_read], b"Hello through symlink!");
        }
    }

    /// Test file symlink removal (removing the symlink itself, not the target)
    #[test_case]
    fn test_file_symlink_removal() {
        use crate::fs::vfs_v2::manager::PathResolutionOptions;
        
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create target file
        vfs.create_file("/target.txt", FileType::RegularFile).unwrap();
        
        // Create symlink to file
        vfs.create_symlink("/file_symlink.txt", "/target.txt").unwrap();
        
        // Verify symlink exists
        let (symlink, _) = vfs.resolve_path_with_options("/file_symlink.txt", &PathResolutionOptions::no_follow()).unwrap();
        assert!(symlink.node().is_symlink().unwrap());
        
        // Remove the symlink (not the target)
        vfs.remove("/file_symlink.txt").unwrap();
        
        // Verify symlink is gone
        let result = vfs.resolve_path_with_options("/file_symlink.txt", &PathResolutionOptions::no_follow());
        assert!(result.is_err(), "Symlink should be removed");
        
        // Verify target file still exists
        let (target, _) = vfs.resolve_path("/target.txt").unwrap();
        assert_eq!(target.node().file_type().unwrap(), FileType::RegularFile);
    }

    /// Test open/read/write operations through file symlink
    #[test_case]
    fn test_file_operations_through_file_symlink() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create target file
        vfs.create_file("/target.txt", FileType::RegularFile).unwrap();
        
        // Create symlink to file
        vfs.create_symlink("/file_symlink.txt", "/target.txt").unwrap();
        
        // Write data through symlink
        let write_file = vfs.open("/file_symlink.txt", 0x02).unwrap(); // Write mode
        if let crate::object::KernelObject::File(file_obj) = write_file {
            file_obj.write(b"Data via file symlink").unwrap();
        }
        
        // Read data through target file
        let read_target = vfs.open("/target.txt", 0x01).unwrap(); // Read mode
        if let crate::object::KernelObject::File(file_obj) = read_target {
            let mut buffer = [0u8; 64];
            let bytes_read = file_obj.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..bytes_read], b"Data via file symlink");
        }
        
        // Write more data through target file (truncate first to overwrite)
        let write_target = vfs.open("/target.txt", 0x02).unwrap(); // Write mode  
        if let crate::object::KernelObject::File(file_obj) = write_target {
            file_obj.truncate(0).unwrap(); // Clear the file first
            file_obj.write(b"Additional data").unwrap();
        }
        
        // Read updated data through symlink
        let read_symlink = vfs.open("/file_symlink.txt", 0x01).unwrap(); // Read mode
        if let crate::object::KernelObject::File(file_obj) = read_symlink {
            let mut buffer = [0u8; 64];
            let bytes_read = file_obj.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..bytes_read], b"Additional data");
        }
    }

    /// Test nested directory symlinks for file operations
    #[test_case]
    fn test_nested_directory_symlink_operations() {
        let tmpfs = TmpFS::new(0);
        let vfs = VfsManager::new_with_root(tmpfs);

        // Create nested directory structure
        vfs.create_dir("/level1").unwrap();
        vfs.create_dir("/level1/level2").unwrap();
        vfs.create_file("/level1/level2/deep_file.txt", FileType::RegularFile).unwrap();
        
        // Create symlinks at different levels
        vfs.create_symlink("/link_level1", "/level1").unwrap();
        vfs.create_symlink("/level1/link_level2", "level2").unwrap(); // Relative path
        
        // Access file through multiple symlinks: /link_level1/link_level2/deep_file.txt
        let (deep_file, _) = vfs.resolve_path("/link_level1/link_level2/deep_file.txt").unwrap();
        assert_eq!(deep_file.node().file_type().unwrap(), FileType::RegularFile);
        
        // Write data through nested symlink path
        let write_file = vfs.open("/link_level1/link_level2/deep_file.txt", 0x02).unwrap();
        if let crate::object::KernelObject::File(file_obj) = write_file {
            file_obj.write(b"Deep symlink data").unwrap();
        }
        
        // Read data through real path
        let read_file = vfs.open("/level1/level2/deep_file.txt", 0x01).unwrap();
        if let crate::object::KernelObject::File(file_obj) = read_file {
            let mut buffer = [0u8; 64];
            let bytes_read = file_obj.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..bytes_read], b"Deep symlink data");
        }
        
        // Create new file through nested symlink path
        vfs.create_file("/link_level1/link_level2/new_deep_file.txt", FileType::RegularFile).unwrap();
        
        // Verify file exists in real location
        let (new_file, _) = vfs.resolve_path("/level1/level2/new_deep_file.txt").unwrap();
        assert_eq!(new_file.node().file_type().unwrap(), FileType::RegularFile);
    }
}
