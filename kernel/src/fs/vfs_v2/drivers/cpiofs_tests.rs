#[cfg(test)]
mod tests {
    use crate::fs::vfs_v2::drivers::cpiofs::CpioFS;
    use crate::fs::{FileType, FileSystemErrorKind};
    use crate::fs::vfs_v2::core::FileSystemOperations;
    use alloc::{vec::Vec, string::ToString, sync::Arc, collections::BTreeSet};

    /// Create a minimal CPIO archive with a symbolic link for testing
    /// 
    /// This creates a CPIO archive in "070701" (new ASCII) format with:
    /// - A regular file: "file.txt" containing "Hello, World!"
    /// - A symbolic link: "link.txt" pointing to "file.txt"
    /// - TRAILER!!! entry to mark end
    fn create_test_cpio_with_symlink() -> Vec<u8> {
        let mut cpio_data = Vec::new();
        
        // Regular file entry: "file.txt"
        let file_content = b"Hello, World!";
        let file_name = b"file.txt\0";
        
        // CPIO header for regular file (mode: 0o100644)
        cpio_data.extend_from_slice(b"070701");           // magic
        cpio_data.extend_from_slice(b"00000001");         // inode
        cpio_data.extend_from_slice(b"000081a4");         // mode (0o100644)
        cpio_data.extend_from_slice(b"00000000");         // uid
        cpio_data.extend_from_slice(b"00000000");         // gid
        cpio_data.extend_from_slice(b"00000001");         // nlink
        cpio_data.extend_from_slice(b"00000000");         // mtime
        cpio_data.extend_from_slice(b"0000000d");         // filesize (13)
        cpio_data.extend_from_slice(b"00000000");         // dev_maj
        cpio_data.extend_from_slice(b"00000000");         // dev_min
        cpio_data.extend_from_slice(b"00000000");         // rdev_maj
        cpio_data.extend_from_slice(b"00000000");         // rdev_min
        cpio_data.extend_from_slice(b"00000009");         // namesize (9)
        cpio_data.extend_from_slice(b"00000000");         // checksum
        
        // File name (padded to 4-byte boundary)
        cpio_data.extend_from_slice(file_name);
        // Add padding to align to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // File content (padded to 4-byte boundary)
        cpio_data.extend_from_slice(file_content);
        // Add padding to align to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // Symbolic link entry: "link.txt" -> "file.txt"
        let symlink_target = b"file.txt";
        let symlink_name = b"link.txt\0";
        
        // CPIO header for symbolic link (mode: 0o120777)
        cpio_data.extend_from_slice(b"070701");           // magic
        cpio_data.extend_from_slice(b"00000002");         // inode
        cpio_data.extend_from_slice(b"0000a1ff");         // mode (0o120777)
        cpio_data.extend_from_slice(b"00000000");         // uid
        cpio_data.extend_from_slice(b"00000000");         // gid
        cpio_data.extend_from_slice(b"00000001");         // nlink
        cpio_data.extend_from_slice(b"00000000");         // mtime
        cpio_data.extend_from_slice(b"00000008");         // filesize (8)
        cpio_data.extend_from_slice(b"00000000");         // dev_maj
        cpio_data.extend_from_slice(b"00000000");         // dev_min
        cpio_data.extend_from_slice(b"00000000");         // rdev_maj
        cpio_data.extend_from_slice(b"00000000");         // rdev_min
        cpio_data.extend_from_slice(b"00000009");         // namesize (9)
        cpio_data.extend_from_slice(b"00000000");         // checksum
        
        // Symlink name (padded to 4-byte boundary)
        cpio_data.extend_from_slice(symlink_name);
        // Add padding to align to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // Symlink target (padded to 4-byte boundary)
        cpio_data.extend_from_slice(symlink_target);
        // Add padding to 4-byte boundary for symlink target
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // TRAILER!!! entry
        cpio_data.extend_from_slice(b"070701");           // magic
        cpio_data.extend_from_slice(b"00000000");         // inode
        cpio_data.extend_from_slice(b"00000000");         // mode
        cpio_data.extend_from_slice(b"00000000");         // uid
        cpio_data.extend_from_slice(b"00000000");         // gid
        cpio_data.extend_from_slice(b"00000001");         // nlink
        cpio_data.extend_from_slice(b"00000000");         // mtime
        cpio_data.extend_from_slice(b"00000000");         // filesize
        cpio_data.extend_from_slice(b"00000000");         // dev_maj
        cpio_data.extend_from_slice(b"00000000");         // dev_min
        cpio_data.extend_from_slice(b"00000000");         // rdev_maj
        cpio_data.extend_from_slice(b"00000000");         // rdev_min
        cpio_data.extend_from_slice(b"0000000b");         // namesize (11)
        cpio_data.extend_from_slice(b"00000000");         // checksum
        
        // TRAILER!!! name (padded to 4-byte boundary)
        cpio_data.extend_from_slice(b"TRAILER!!!\0");
        
        cpio_data
    }

    /// Create a CPIO archive with a directory and symlink to test directory symlinks
    /// 
    /// This creates a CPIO archive with:
    /// - A directory: "testdir/"
    /// - A regular file: "testdir/file.txt" containing "Hello, Dir!"
    /// - A symbolic link: "linkdir" pointing to "testdir"
    /// - TRAILER!!! entry to mark end
    fn create_test_cpio_with_dir_symlink() -> Vec<u8> {
        let mut cpio_data = Vec::new();
        
        // Directory entry: "testdir"
        let dir_name = b"testdir\0";
        
        // CPIO header for directory (mode: 0o040755)
        cpio_data.extend_from_slice(b"070701");           // magic
        cpio_data.extend_from_slice(b"00000001");         // inode
        cpio_data.extend_from_slice(b"000041ed");         // mode (0o040755)
        cpio_data.extend_from_slice(b"00000000");         // uid
        cpio_data.extend_from_slice(b"00000000");         // gid
        cpio_data.extend_from_slice(b"00000002");         // nlink
        cpio_data.extend_from_slice(b"00000000");         // mtime
        cpio_data.extend_from_slice(b"00000000");         // filesize (0 for directory)
        cpio_data.extend_from_slice(b"00000000");         // dev_maj
        cpio_data.extend_from_slice(b"00000000");         // dev_min
        cpio_data.extend_from_slice(b"00000000");         // rdev_maj
        cpio_data.extend_from_slice(b"00000000");         // rdev_min
        cpio_data.extend_from_slice(b"00000008");         // namesize (8)
        cpio_data.extend_from_slice(b"00000000");         // checksum
        
        // Directory name (padded to 4-byte boundary)
        cpio_data.extend_from_slice(dir_name);
        // Add padding to align to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // Regular file entry: "testdir/file.txt"
        let file_content = b"Hello, Dir!";
        let file_name = b"testdir/file.txt\0";
        
        // CPIO header for regular file (mode: 0o100644)
        cpio_data.extend_from_slice(b"070701");           // magic
        cpio_data.extend_from_slice(b"00000002");         // inode
        cpio_data.extend_from_slice(b"000081a4");         // mode (0o100644)
        cpio_data.extend_from_slice(b"00000000");         // uid
        cpio_data.extend_from_slice(b"00000000");         // gid
        cpio_data.extend_from_slice(b"00000001");         // nlink
        cpio_data.extend_from_slice(b"00000000");         // mtime
        cpio_data.extend_from_slice(b"0000000c");         // filesize (12)
        cpio_data.extend_from_slice(b"00000000");         // dev_maj
        cpio_data.extend_from_slice(b"00000000");         // dev_min
        cpio_data.extend_from_slice(b"00000000");         // rdev_maj
        cpio_data.extend_from_slice(b"00000000");         // rdev_min
        cpio_data.extend_from_slice(b"00000011");         // namesize (17)
        cpio_data.extend_from_slice(b"00000000");         // checksum
        
        // File name (padded to 4-byte boundary)
        cpio_data.extend_from_slice(file_name);
        // Add padding to align to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // File content (padded to 4-byte boundary)
        cpio_data.extend_from_slice(file_content);
        // Add padding to align to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // Symbolic link entry: "linkdir" -> "testdir"
        let symlink_target = b"testdir";
        let symlink_name = b"linkdir\0";
        
        // CPIO header for symbolic link (mode: 0o120777)
        cpio_data.extend_from_slice(b"070701");           // magic
        cpio_data.extend_from_slice(b"00000003");         // inode
        cpio_data.extend_from_slice(b"0000a1ff");         // mode (0o120777)
        cpio_data.extend_from_slice(b"00000000");         // uid
        cpio_data.extend_from_slice(b"00000000");         // gid
        cpio_data.extend_from_slice(b"00000001");         // nlink
        cpio_data.extend_from_slice(b"00000000");         // mtime
        cpio_data.extend_from_slice(b"00000007");         // filesize (7)
        cpio_data.extend_from_slice(b"00000000");         // dev_maj
        cpio_data.extend_from_slice(b"00000000");         // dev_min
        cpio_data.extend_from_slice(b"00000000");         // rdev_maj
        cpio_data.extend_from_slice(b"00000000");         // rdev_min
        cpio_data.extend_from_slice(b"00000008");         // namesize (8)
        cpio_data.extend_from_slice(b"00000000");         // checksum
        
        // Symlink name (padded to 4-byte boundary)
        cpio_data.extend_from_slice(symlink_name);
        // Add padding to align to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // Symlink target (padded to 4-byte boundary)
        cpio_data.extend_from_slice(symlink_target);
        // Add padding to 4-byte boundary
        while cpio_data.len() % 4 != 0 {
            cpio_data.push(0);
        }
        
        // TRAILER!!! entry
        cpio_data.extend_from_slice(b"070701");           // magic
        cpio_data.extend_from_slice(b"00000000");         // inode
        cpio_data.extend_from_slice(b"00000000");         // mode
        cpio_data.extend_from_slice(b"00000000");         // uid
        cpio_data.extend_from_slice(b"00000000");         // gid
        cpio_data.extend_from_slice(b"00000001");         // nlink
        cpio_data.extend_from_slice(b"00000000");         // mtime
        cpio_data.extend_from_slice(b"00000000");         // filesize
        cpio_data.extend_from_slice(b"00000000");         // dev_maj
        cpio_data.extend_from_slice(b"00000000");         // dev_min
        cpio_data.extend_from_slice(b"00000000");         // rdev_maj
        cpio_data.extend_from_slice(b"00000000");         // rdev_min
        cpio_data.extend_from_slice(b"0000000b");         // namesize (11)
        cpio_data.extend_from_slice(b"00000000");         // checksum
        
        // TRAILER!!! name (padded to 4-byte boundary)
        cpio_data.extend_from_slice(b"TRAILER!!!\0");
        
        cpio_data
    }

    /// Test basic CpioFS symlink parsing and reading
    #[test_case]
    fn test_cpiofs_symlink_basic() {
        // Create test CPIO data with symlink
        let cpio_data = create_test_cpio_with_symlink();
        
        // Create CpioFS from the data
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        // Get root and look for the symlink
        let root_node = cpiofs.root_node();
        let symlink_node = cpiofs.lookup(&root_node, &"link.txt".to_string()).unwrap();
        
        // Verify it's a symbolic link
        assert!(symlink_node.is_symlink().unwrap());
        assert!(matches!(symlink_node.file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Read the target
        let target = symlink_node.read_link().unwrap();
        assert_eq!(target, "file.txt");
    }

    /// Test symlink metadata in CpioFS
    #[test_case]
    fn test_cpiofs_symlink_metadata() {
        let cpio_data = create_test_cpio_with_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let symlink_node = cpiofs.lookup(&root_node, &"link.txt".to_string()).unwrap();
        
        let metadata = symlink_node.metadata().unwrap();
        
        // Check metadata
        assert!(matches!(metadata.file_type, FileType::SymbolicLink(_)));
        assert_eq!(metadata.size, 8); // Length of "file.txt"
        assert_eq!(metadata.link_count, 1);
        assert!(metadata.permissions.read);
        assert!(!metadata.permissions.write); // CpioFS is read-only
        assert!(metadata.permissions.execute);
    }

    /// Test reading link from non-symlink in CpioFS returns error
    #[test_case]
    fn test_cpiofs_read_link_error() {
        let cpio_data = create_test_cpio_with_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        
        // Try to read link from regular file
        let file_node = cpiofs.lookup(&root_node, &"file.txt".to_string()).unwrap();
        let result = file_node.read_link();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::NotSupported));
        }
        
        // Try to read link from directory (root)
        let result = root_node.read_link();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::NotSupported));
        }
    }

    /// Test directory listing includes symlinks with correct type
    #[test_case]
    fn test_cpiofs_readdir_symlink() {
        let cpio_data = create_test_cpio_with_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let entries = cpiofs.readdir(&root_node).unwrap();
        
        // Should have ".", "..", "file.txt", and "link.txt"
        assert_eq!(entries.len(), 4);
        
        // Find the symlink entry
        let symlink_entry = entries.iter().find(|e| e.name == "link.txt").unwrap();
        assert!(matches!(symlink_entry.file_type, FileType::SymbolicLink(_)));
        
        // Find the regular file entry
        let file_entry = entries.iter().find(|e| e.name == "file.txt").unwrap();
        assert_eq!(file_entry.file_type, FileType::RegularFile);
    }

    /// Test basic CpioFS directory symlink parsing and reading
    #[test_case]
    fn test_cpiofs_dir_symlink_basic() {
        // Create test CPIO data with directory symlink
        let cpio_data = create_test_cpio_with_dir_symlink();
        
        // Create CpioFS from the data
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        // Get root and look for the directory symlink
        let root_node = cpiofs.root_node();
        let dir_symlink_node = cpiofs.lookup(&root_node, &"linkdir".to_string()).unwrap();
        
        // Verify it's a symbolic link
        assert!(dir_symlink_node.is_symlink().unwrap());
        assert!(matches!(dir_symlink_node.file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Read the target
        let target = dir_symlink_node.read_link().unwrap();
        assert_eq!(target, "testdir");
    }

    /// Test directory symlink metadata in CpioFS
    #[test_case]
    fn test_cpiofs_dir_symlink_metadata() {
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let dir_symlink_node = cpiofs.lookup(&root_node, &"linkdir".to_string()).unwrap();
        
        let metadata = dir_symlink_node.metadata().unwrap();
        
        // Check metadata
        assert!(matches!(metadata.file_type, FileType::SymbolicLink(_)));
        assert_eq!(metadata.size, 7); // Length of "testdir"
        assert_eq!(metadata.link_count, 1);
        assert!(metadata.permissions.read);
        assert!(!metadata.permissions.write); // CpioFS is read-only
        assert!(metadata.permissions.execute);
    }

    /// Test reading link from non-symlink in CpioFS returns error (directory symlink)
    #[test_case]
    fn test_cpiofs_read_dir_link_error() {
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        
        // First lookup testdir (the actual directory), then lookup file.txt within testdir
        let testdir_node = cpiofs.lookup(&root_node, &"testdir".to_string()).unwrap();
        let file_node = cpiofs.lookup(&testdir_node, &"file.txt".to_string()).unwrap();
        
        // Try to read link from regular file inside directory
        let result = file_node.read_link();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::NotSupported));
        }
        
        // Try to read link from actual directory (not symlink)
        let result = testdir_node.read_link();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(matches!(e.kind, FileSystemErrorKind::NotSupported));
        }
        
        // Now test that the directory symlink DOES work
        let dir_symlink_node = cpiofs.lookup(&root_node, &"linkdir".to_string()).unwrap();
        let result = dir_symlink_node.read_link();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "testdir");
    }

    /// Test directory listing includes directory symlinks with correct type
    #[test_case]
    fn test_cpiofs_readdir_dir_symlink() {
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let entries = cpiofs.readdir(&root_node).unwrap();
        
        // Should have ".", "..", "testdir/", and "linkdir"
        assert_eq!(entries.len(), 4);
        
        // Find the directory symlink entry
        let dir_symlink_entry = entries.iter().find(|e| e.name == "linkdir").unwrap();
        assert!(matches!(dir_symlink_entry.file_type, FileType::SymbolicLink(_)));
        
        // Find the directory entry
        let dir_entry = entries.iter().find(|e| e.name == "testdir").unwrap();
        assert_eq!(dir_entry.file_type, FileType::Directory);
    }

    /// Test directory symlink basic functionality
    #[test_case]
    fn test_cpiofs_directory_symlink() {
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        
        // Check that directory symlink exists
        let symlink_node = cpiofs.lookup(&root_node, &"linkdir".to_string()).unwrap();
        let target = symlink_node.read_link().unwrap();
        assert_eq!(target, "testdir");
        
        // Check symlink metadata
        let metadata = symlink_node.metadata().unwrap();
        assert!(matches!(metadata.file_type, FileType::SymbolicLink(_)));
        
        // Check that original directory exists
        let dir_node = cpiofs.lookup(&root_node, &"testdir".to_string()).unwrap();
        let dir_metadata = dir_node.metadata().unwrap();
        assert_eq!(dir_metadata.file_type, FileType::Directory);
        
        // Check that file in directory exists
        let file_node = cpiofs.lookup(&dir_node, &"file.txt".to_string()).unwrap();
        let file_metadata = file_node.metadata().unwrap();
        assert_eq!(file_metadata.file_type, FileType::RegularFile);
    }

    /// Test directory listing with directory symlinks
    #[test_case]
    fn test_cpiofs_readdir_with_directory_symlink() {
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let entries = cpiofs.readdir(&root_node).unwrap();
        
        // Should have ".", "..", "testdir", and "linkdir"
        assert_eq!(entries.len(), 4);
        
        // Find the symlink entry
        let symlink_entry = entries.iter().find(|e| e.name == "linkdir").unwrap();
        assert!(matches!(symlink_entry.file_type, FileType::SymbolicLink(_)));
        
        // Find the directory entry
        let dir_entry = entries.iter().find(|e| e.name == "testdir").unwrap();
        assert_eq!(dir_entry.file_type, FileType::Directory);
        
        // Check directory contents
        let dir_node = cpiofs.lookup(&root_node, &"testdir".to_string()).unwrap();
        let dir_entries = cpiofs.readdir(&dir_node).unwrap();
        
        // Should have ".", "..", and "file.txt"
        assert_eq!(dir_entries.len(), 3);
        
        let file_entry = dir_entries.iter().find(|e| e.name == "file.txt").unwrap();
        assert_eq!(file_entry.file_type, FileType::RegularFile);
    }

    /// Test VFS-level symlink resolution for directory symlinks
    #[test_case]
    fn test_vfs_directory_symlink_resolution() {
        use crate::fs::vfs_v2::manager::VfsManager;
        
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        // Create VfsManager and mount the filesystem
        let vfs = Arc::new(VfsManager::new_with_root(cpiofs));
        
        // Test that we can resolve files through the directory symlink using VFS
        let result = vfs.resolve_path("/linkdir/file.txt");
        assert!(result.is_ok(), "Should be able to resolve file through directory symlink");
        
        let (entry, _mount_point) = result.unwrap();
        let metadata = entry.node().metadata().unwrap();
        assert_eq!(metadata.file_type, FileType::RegularFile);
        
        // Test that direct access also works
        let result = vfs.resolve_path("/testdir/file.txt");
        assert!(result.is_ok(), "Should be able to resolve file directly");
        
        // Test symlink itself
        let result = vfs.resolve_path("/linkdir");
        assert!(result.is_ok(), "Should be able to resolve directory symlink");
        
        let (entry, _mount_point) = result.unwrap();
        // The resolved entry should point to the target directory, not the symlink itself
        assert_eq!(entry.node().file_type().unwrap(), FileType::Directory);
    }

    /// Test VFS-level directory listing through symlinks
    #[test_case]
    fn test_vfs_readdir_through_symlink() {
        use crate::fs::vfs_v2::manager::VfsManager;
        
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        // Create VfsManager and mount the filesystem
        let vfs = Arc::new(VfsManager::new_with_root(cpiofs));
        
        // Test directory listing through symlink
        let result = vfs.readdir("/linkdir");
        assert!(result.is_ok(), "Should be able to list directory through symlink");
        
        let entries = result.unwrap();
        
        // Should contain the files in the target directory
        assert!(entries.iter().any(|e| e.name == "file.txt"), "Should contain file.txt");
        
        // Compare with direct listing
        let direct_result = vfs.readdir("/testdir");
        assert!(direct_result.is_ok());
        let direct_entries = direct_result.unwrap();
        
        // The listings should be equivalent (ignoring . and ..)
        let symlink_files: BTreeSet<_> = entries.iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| &e.name)
            .collect();
        let direct_files: BTreeSet<_> = direct_entries.iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| &e.name)
            .collect();
        
        assert_eq!(symlink_files, direct_files, "Directory listings should be equivalent");
    }

    /// Test VFS path resolution with O_NOFOLLOW equivalent behavior
    #[test_case]
    fn test_vfs_path_resolution_no_follow() {
        use crate::fs::vfs_v2::manager::{VfsManager, PathResolutionOptions};
        
        let cpio_data = create_test_cpio_with_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        // Create VfsManager and mount the filesystem
        let vfs = Arc::new(VfsManager::new_with_root(cpiofs));
        
        // Test normal resolution (follows symlinks)
        let result = vfs.resolve_path("/link.txt");
        assert!(result.is_ok(), "Should be able to resolve symlink normally");
        let (entry, _mount_point) = result.unwrap();
        // Should resolve to the target file
        assert_eq!(entry.node().file_type().unwrap(), FileType::RegularFile);
        
        // Test no-follow resolution (doesn't follow symlinks)
        let result = vfs.resolve_path_with_options("/link.txt", &PathResolutionOptions::no_follow());
        assert!(result.is_ok(), "Should be able to resolve symlink with no_follow");
        let (entry, _mount_point) = result.unwrap();
        // Should return the symlink itself, not the target
        assert!(matches!(entry.node().file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Verify we can read the link target
        let target = entry.node().read_link().unwrap();
        assert_eq!(target, "file.txt");
    }

    /// Test VFS path resolution with no_follow (lstat-like behavior)
    #[test_case]
    fn test_vfs_path_resolution_no_follow() {
        use crate::fs::vfs_v2::manager::{VfsManager, PathResolutionOptions};
        
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        // Create VfsManager and mount the filesystem
        let vfs = Arc::new(VfsManager::new_with_root(cpiofs));
        
        // Test accessing a file through a directory symlink with no_follow
        // This should follow intermediate symlinks but not the final component
        
        // First test: linkdir itself (final component is a symlink)
        let result = vfs.resolve_path_with_options("/linkdir", &PathResolutionOptions::no_follow());
        assert!(result.is_ok(), "Should be able to resolve directory symlink with no_follow");
        let (entry, _mount_point) = result.unwrap();
        // Should return the symlink itself
        assert!(matches!(entry.node().file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Second test: normal resolution for comparison
        let result = vfs.resolve_path("/linkdir");
        assert!(result.is_ok(), "Should be able to resolve directory symlink normally");
        let (entry, _mount_point) = result.unwrap();
        // Should resolve to the target directory
        assert_eq!(entry.node().file_type().unwrap(), FileType::Directory);
    }

    /// Test opening symbolic links directly
    #[test_case]
    fn test_cpiofs_open_symlink() {
        let cpio_data = create_test_cpio_with_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let symlink_node = cpiofs.lookup(&root_node, &"link.txt".to_string()).unwrap();
        
        // Verify it's a symbolic link
        assert!(matches!(symlink_node.file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Open the symbolic link
        let symlink_file = cpiofs.open(&symlink_node, 0).unwrap();
        
        // Read from the symbolic link (should return the target path)
        let mut buffer = [0u8; 64];
        let bytes_read = symlink_file.read(&mut buffer).unwrap();
        let content = core::str::from_utf8(&buffer[..bytes_read]).unwrap();
        assert_eq!(content, "file.txt");
        
        // Verify metadata
        let metadata = symlink_file.metadata().unwrap();
        assert!(matches!(metadata.file_type, FileType::SymbolicLink(_)));
        assert_eq!(metadata.size, 8); // Length of "file.txt"
    }

    /// Test opening directory symbolic links directly  
    #[test_case]
    fn test_cpiofs_open_dir_symlink() {
        let cpio_data = create_test_cpio_with_dir_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let dir_symlink_node = cpiofs.lookup(&root_node, &"linkdir".to_string()).unwrap();
        
        // Verify it's a symbolic link
        assert!(matches!(dir_symlink_node.file_type().unwrap(), FileType::SymbolicLink(_)));
        
        // Open the directory symbolic link
        let symlink_file = cpiofs.open(&dir_symlink_node, 0).unwrap();
        
        // Read from the symbolic link (should return the target path)
        let mut buffer = [0u8; 64];
        let bytes_read = symlink_file.read(&mut buffer).unwrap();
        let content = core::str::from_utf8(&buffer[..bytes_read]).unwrap();
        assert_eq!(content, "testdir");
        
        // Verify metadata
        let metadata = symlink_file.metadata().unwrap();
        assert!(matches!(metadata.file_type, FileType::SymbolicLink(_)));
        assert_eq!(metadata.size, 7); // Length of "testdir"
    }

    /// Test seek operations on symbolic links
    #[test_case]
    fn test_cpiofs_symlink_seek() {
        let cpio_data = create_test_cpio_with_symlink();
        let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).unwrap();
        
        let root_node = cpiofs.root_node();
        let symlink_node = cpiofs.lookup(&root_node, &"link.txt".to_string()).unwrap();
        let symlink_file = cpiofs.open(&symlink_node, 0).unwrap();
        
        // Test seeking to start
        let pos = symlink_file.seek(crate::fs::SeekFrom::Start(0)).unwrap();
        assert_eq!(pos, 0);
        
        // Test seeking to end
        let pos = symlink_file.seek(crate::fs::SeekFrom::End(0)).unwrap();
        assert_eq!(pos, 8); // Length of "file.txt"
        
        // Test seeking relative
        let pos = symlink_file.seek(crate::fs::SeekFrom::Current(-4)).unwrap();
        assert_eq!(pos, 4);
        
        // Read from middle
        let mut buffer = [0u8; 4];
        let bytes_read = symlink_file.read(&mut buffer).unwrap();
        let content = core::str::from_utf8(&buffer[..bytes_read]).unwrap();
        assert_eq!(content, ".txt");
    }
}