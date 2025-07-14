#[cfg(test)]
mod tests {
    use crate::fs::vfs_v2::drivers::cpiofs::CpioFS;
    use crate::fs::{FileType, FileSystemErrorKind};
    use crate::fs::vfs_v2::core::FileSystemOperations;
    use alloc::{vec::Vec, string::ToString};

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
        assert!(!metadata.permissions.execute);
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
}