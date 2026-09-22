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
fn ext2_last_link_stays_intact_until_final_open_description_drops() {
    let fs = unlink_fixture(false);
    let root = fs.root_node();
    let node = fs.lookup(&root, &"file".to_string()).unwrap();
    let first = fs.open(&node, 0).unwrap();
    // A separately resolved node must participate in the same inode count.
    let alias_node = fs.lookup(&root, &"file".to_string()).unwrap();
    let second = fs.open(&alias_node, 0).unwrap();
    let duplicate = first.clone();
    for retained in [first, duplicate, second] {
        assert_eq!(
            fs.remove(&root, &"file".to_string()).unwrap_err().kind,
            FileSystemErrorKind::Busy
        );
        assert_eq!(fs.lookup(&root, &"file".to_string()).unwrap().id(), 11);
        assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 1);
        drop(retained);
    }
    fs.remove(&root, &"file".to_string()).unwrap();
    assert_eq!(
        fs.lookup(&root, &"file".to_string()).unwrap_err().kind,
        FileSystemErrorKind::NotFound
    );
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
    assert_eq!(
        fs.remove(&root, &"alias".to_string()).unwrap_err().kind,
        FileSystemErrorKind::Busy
    );
    drop(opened);
    fs.remove(&root, &"alias".to_string()).unwrap();
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
fn ext2_empty_directory_removal_is_explicitly_unsupported_and_preserves_cwd() {
    let fs = unlink_fixture(false);
    let mut directory = fs.read_inode(11).unwrap();
    directory.mode = (EXT2_S_IFDIR | 0o755).to_le();
    directory.links_count = 2_u16.to_le();
    directory.size = 0;
    fs.write_inode(11, &directory).unwrap();
    let original = crate::fs::vfs_v2::manager::VfsManager::new_with_root(fs.clone());
    let independent = crate::fs::vfs_v2::manager::VfsManager::new_with_root(fs.clone());
    independent.set_cwd_by_path("/file").unwrap();
    assert_eq!(
        original.remove_with_kind("/file", true).unwrap_err().kind,
        FileSystemErrorKind::NotSupported
    );
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
    assert_eq!(fs.read_inode(11).unwrap().get_links_count(), 2);
    assert!(fs.lookup(&fs.root_node(), &"file".to_string()).is_ok());
}
