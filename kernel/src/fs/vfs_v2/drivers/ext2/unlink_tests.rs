use super::*;

fn directory_block(entries: &[(u32, &str, u8)]) -> Vec<u8> {
    let mut bytes = vec![0; 1024];
    let mut offset = 0;
    for (index, &(inode, name, kind)) in entries.iter().enumerate() {
        let length = if index + 1 == entries.len() {
            1024 - offset
        } else {
            (8 + name.len() + 3) & !3
        };
        bytes[offset..offset + 4].copy_from_slice(&inode.to_le_bytes());
        bytes[offset + 4..offset + 6].copy_from_slice(&(length as u16).to_le_bytes());
        bytes[offset + 6] = name.len() as u8;
        bytes[offset + 7] = kind;
        bytes[offset + 8..offset + 8 + name.len()].copy_from_slice(name.as_bytes());
        offset += length;
    }
    bytes
}

fn unlink_fixture(two_links: bool) -> Arc<Ext2FileSystem> {
    let (device, fs, _) = create_writeback_test_file();
    let mut descriptors = vec![0; 1024];
    descriptors[0..4].copy_from_slice(&3_u32.to_le_bytes());
    descriptors[4..8].copy_from_slice(&4_u32.to_le_bytes());
    descriptors[8..12].copy_from_slice(&5_u32.to_le_bytes());
    device.enqueue_request(Box::new(BlockIORequest {
        request_type: BlockIORequestType::Write,
        sector: 4,
        sector_count: 2,
        head: 0,
        cylinder: 0,
        buffer: descriptors,
    }));
    assert!(device.process_requests()[0].result.is_ok());
    let mut bitmap = vec![0; 1024];
    bitmap[1 / 8] |= 1 << (1 % 8); // root inode 2
    bitmap[10 / 8] |= 1 << (10 % 8); // file inode 11
    fs.write_block_cached(4, &bitmap).unwrap();
    bitmap.fill(0);
    bitmap[299 / 8] |= 1 << (299 % 8); // data block 300
    bitmap[300 / 8] |= 1 << (300 % 8); // root block 301
    fs.write_block_cached(3, &bitmap).unwrap();
    let mut root = Ext2Inode::empty();
    root.mode = (EXT2_S_IFDIR | 0o755).to_le();
    root.size = 1024_u32.to_le();
    root.links_count = 2_u16.to_le();
    root.block[0] = 301_u32.to_le();
    fs.write_inode(2, &root).unwrap();
    let mut file = fs.read_inode(11).unwrap();
    file.links_count = (if two_links { 2_u16 } else { 1_u16 }).to_le();
    fs.write_inode(11, &file).unwrap();
    let entries = if two_links {
        directory_block(&[(11, "file", 1), (11, "alias", 1)])
    } else {
        directory_block(&[(11, "file", 1)])
    };
    fs.write_block_cached(301, &entries).unwrap();
    fs
}

#[test_case]
fn ext2_unlink_detaches_name_and_preserves_all_live_descriptions_and_nodes() {
    let fs = unlink_fixture(false);
    let root = fs.root_node();
    let node = fs.lookup(&root, &"file".to_string()).unwrap();
    let first = fs.open(&node, 0).unwrap();
    // A separately resolved node must participate in the same inode count.
    let alias_node = fs.lookup(&root, &"file".to_string()).unwrap();
    let second = fs.open(&alias_node, 0).unwrap();
    let duplicate = first.clone();
    fs.remove(&root, &"file".to_string()).unwrap();
    assert_eq!(
        fs.lookup(&root, &"file".to_string()).unwrap_err().kind,
        FileSystemErrorKind::NotFound
    );
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 0);
    for retained in [first, duplicate, second] {
        assert_eq!(retained.metadata().unwrap().size, 64);
        fs.reclaim_unlinked_inodes().unwrap();
        assert_ne!(fs.read_inode(11).unwrap().get_mode(), 0);
        drop(retained);
    }
    // Retained nodes, even without fds, also prevent inode reuse.
    fs.reclaim_unlinked_inodes().unwrap();
    assert_ne!(fs.read_inode(11).unwrap().get_mode(), 0);
    drop(node);
    drop(alias_node);
    fs.reclaim_unlinked_inodes().unwrap();
    assert_eq!(fs.read_inode(11).unwrap().get_mode(), 0);
}

#[test_case]
fn ext2_unlink_preserves_other_hardlinks_and_live_descriptions() {
    let fs = unlink_fixture(true);
    let root = fs.root_node();
    let node = fs.lookup(&root, &"alias".to_string()).unwrap();
    let opened = fs.open(&node, 0).unwrap();
    fs.remove(&root, &"file".to_string()).unwrap();
    assert_eq!(
        fs.lookup(&root, &"file".to_string()).unwrap_err().kind,
        FileSystemErrorKind::NotFound
    );
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 1);
    assert_eq!(opened.metadata().unwrap().size, 64);
    fs.remove(&root, &"alias".to_string()).unwrap();
    assert_eq!(opened.metadata().unwrap().size, 64);
    drop(opened);
    drop(node);
    fs.reclaim_unlinked_inodes().unwrap();
    assert_eq!(fs.read_inode(11).unwrap().get_mode(), 0);
}

#[test_case]
fn ext2_nonempty_directory_removal_preserves_directory_entry() {
    let fs = unlink_fixture(false);
    let mut directory = fs.read_inode(11).unwrap();
    directory.mode = (EXT2_S_IFDIR | 0o755).to_le();
    directory.links_count = 2_u16.to_le();
    directory.size = 1024_u32.to_le();
    fs.write_inode(11, &directory).unwrap();
    fs.write_block_cached(300, &directory_block(&[(2, "child", 2)]))
        .unwrap();
    let root = fs.root_node();
    assert_eq!(
        fs.remove(&root, &"file".to_string()).unwrap_err().kind,
        FileSystemErrorKind::DirectoryNotEmpty
    );
    assert!(
        fs.lookup(&root, &"file".to_string())
            .unwrap()
            .is_directory()
            .unwrap()
    );
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 2);
}

#[test_case]
fn ext2_removed_directory_retains_cwd_inode_until_namespace_releases_it() {
    let fs = unlink_fixture(false);
    let mut directory = fs.read_inode(11).unwrap();
    directory.mode = (EXT2_S_IFDIR | 0o755).to_le();
    directory.links_count = 2_u16.to_le();
    directory.size = 0;
    fs.write_inode(11, &directory).unwrap();
    let original = crate::fs::vfs_v2::manager::VfsManager::new_with_root(fs.clone());
    let independent = crate::fs::vfs_v2::manager::VfsManager::new_with_root(fs.clone());
    independent.set_cwd_by_path("/file").unwrap();
    original.remove_with_kind("/file", true).unwrap();
    assert_eq!(independent.get_cwd_path(), "/file");
    assert!(
        independent
            .resolve_path(".")
            .unwrap()
            .0
            .node()
            .is_directory()
            .unwrap()
    );
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 0);
    assert_eq!(fs.read_inode(2).unwrap().get_links_count(), 1);
    assert_eq!(
        fs.lookup(&fs.root_node(), &"file".to_string())
            .unwrap_err()
            .kind,
        FileSystemErrorKind::NotFound
    );
    fs.reclaim_unlinked_inodes().unwrap();
    assert_ne!(fs.read_inode(11).unwrap().get_mode(), 0);
    assert_eq!(
        independent
            .create_file("child", FileType::RegularFile)
            .unwrap_err()
            .kind,
        FileSystemErrorKind::NotFound
    );
    drop(independent);
    drop(original);
    fs.reclaim_unlinked_inodes().unwrap();
    assert_eq!(fs.read_inode(11).unwrap().get_mode(), 0);
}

#[test_case]
fn ext2_new_hardlink_shares_inode_and_survives_source_unlink() {
    let fs = unlink_fixture(false);
    let root = fs.root_node();
    let source = fs.lookup(&root, &"file".to_string()).unwrap();
    let link = fs
        .create_hardlink(&root, &"linked".to_string(), &source)
        .unwrap();
    assert_eq!(source.id(), link.id());
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 2);
    assert_eq!(
        fs.create_hardlink(&root, &"linked".to_string(), &source)
            .unwrap_err()
            .kind,
        FileSystemErrorKind::FileExists
    );
    fs.rename(&root, &"file".to_string(), &root, &"linked".to_string())
        .unwrap();
    assert!(fs.lookup(&root, &"file".to_string()).is_ok());
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 2);
    fs.remove(&root, &"file".to_string()).unwrap();
    assert_eq!(fs.lookup(&root, &"linked".to_string()).unwrap().id(), 11);
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 1);
}

#[test_case]
fn ext2_rename_replaces_open_destination_without_reusing_its_inode() {
    let fs = unlink_fixture(false);
    let root = fs.root_node();
    let original = fs.lookup(&root, &"file".to_string()).unwrap();
    let opened = fs.open(&original, 0).unwrap();
    // This fixture has no allocator free-count metadata. Supply a second
    // allocated inode directly so the test isolates rename and reclamation.
    let mut inode = Ext2Inode::empty();
    inode.mode = (EXT2_S_IFREG | 0o644).to_le();
    inode.links_count = 1_u16.to_le();
    fs.write_inode(12, &inode).unwrap();
    fs.write_block_cached(301, &directory_block(&[(11, "file", 1), (12, "new", 1)]))
        .unwrap();
    let replacement = fs.lookup(&root, &"new".to_string()).unwrap();
    fs.rename(&root, &"new".to_string(), &root, &"file".to_string())
        .unwrap();
    assert_eq!(opened.metadata().unwrap().size, 64);
    assert_eq!(
        fs.lookup(&root, &"file".to_string()).unwrap().id(),
        replacement.id()
    );
    assert_ne!(replacement.id(), original.id());
    fs.reclaim_unlinked_inodes().unwrap();
    assert_ne!(fs.read_inode(11).unwrap().get_mode(), 0);
    drop(opened);
    drop(original);
    fs.reclaim_unlinked_inodes().unwrap();
    assert_eq!(fs.read_inode(11).unwrap().get_mode(), 0);
}
