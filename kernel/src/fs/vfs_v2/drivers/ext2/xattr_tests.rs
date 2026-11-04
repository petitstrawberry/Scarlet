use super::*;
use crate::fs::vfs_v2::core::FileSystemOperations;
use crate::fs::vfs_v2::xattr::{self, Error as XattrError, SetMode};

const TARGET_INODE: u32 = 2;
const INODE_TABLE_BLOCK: u64 = 5;
const BLOCK_BITMAP_BLOCK: u64 = 3;
const INODE_BITMAP_BLOCK: u64 = 4;

fn read_block(device: &MockBlockDevice, block: u64) -> Vec<u8> {
    let result = device.submit_requests(vec![Box::new(BlockIORequest {
        request_type: BlockIORequestType::Read,
        sector: block as usize * 2,
        sector_count: 2,
        head: 0,
        cylinder: 0,
        buffer: vec![0; 1024],
    })]);
    assert_eq!(result.len(), 1);
    assert!(result[0].result.is_ok());
    result[0].request.buffer.clone()
}

fn write_block(device: &MockBlockDevice, block: u64, buffer: Vec<u8>) {
    let result = device.submit_requests(vec![Box::new(BlockIORequest {
        request_type: BlockIORequestType::Write,
        sector: block as usize * 2,
        sector_count: 2,
        head: 0,
        cylinder: 0,
        buffer,
    })]);
    assert_eq!(result.len(), 1);
    assert!(result[0].result.is_ok());
}

fn bitmap_has(device: &MockBlockDevice, bitmap: u64, bit: usize) -> bool {
    read_block(device, bitmap)[bit / 8] & (1 << (bit % 8)) != 0
}

fn set_inode_allocated(device: &MockBlockDevice, inode: u32) {
    let mut bitmap = read_block(device, INODE_BITMAP_BLOCK);
    let bit = (inode - 1) as usize;
    bitmap[bit / 8] |= 1 << (bit % 8);
    write_block(device, INODE_BITMAP_BLOCK, bitmap);
}

fn xattr_fixture() -> Arc<MockBlockDevice> {
    let device = Arc::new(create_test_ext2_device());

    let mut superblock = read_block(&device, 1);
    superblock[12..16].copy_from_slice(&7931_u32.to_le_bytes());
    superblock[16..20].copy_from_slice(&2047_u32.to_le_bytes());
    write_block(&device, 1, superblock);

    let mut descriptors = vec![0; 1024];
    descriptors[0..4].copy_from_slice(&(BLOCK_BITMAP_BLOCK as u32).to_le_bytes());
    descriptors[4..8].copy_from_slice(&(INODE_BITMAP_BLOCK as u32).to_le_bytes());
    descriptors[8..12].copy_from_slice(&(INODE_TABLE_BLOCK as u32).to_le_bytes());
    descriptors[12..14].copy_from_slice(&7931_u16.to_le_bytes());
    descriptors[14..16].copy_from_slice(&2047_u16.to_le_bytes());
    write_block(&device, 2, descriptors);

    let mut block_bitmap = vec![0; 1024];
    // Blocks one through the 256-block inode table are metadata.
    for block in 1..=260usize {
        let bit = block - 1;
        block_bitmap[bit / 8] |= 1 << (bit % 8);
    }
    write_block(&device, BLOCK_BITMAP_BLOCK, block_bitmap);
    set_inode_allocated(&device, TARGET_INODE);

    let mut table = vec![0; 1024];
    let inode_offset = ((TARGET_INODE - 1) * 128) as usize;
    let mut inode = Ext2Inode::empty();
    inode.mode = (EXT2_S_IFREG | 0o644).to_le();
    inode.links_count = 1_u16.to_le();
    let inode_bytes = unsafe {
        core::slice::from_raw_parts(
            &inode as *const Ext2Inode as *const u8,
            core::mem::size_of::<Ext2Inode>(),
        )
    };
    table[inode_offset..inode_offset + inode_bytes.len()].copy_from_slice(inode_bytes);
    write_block(&device, INODE_TABLE_BLOCK, table);
    device
}

fn xattr_node(fs: &Arc<Ext2FileSystem>) -> Arc<Ext2Node> {
    let node = Arc::new(Ext2Node::new(TARGET_INODE, FileType::RegularFile, 1));
    node.set_filesystem(Arc::downgrade(&(fs.clone() as Arc<dyn FileSystemOperations>)));
    node
}

#[test_case]
fn ext2_xattr_persists_across_remount_and_reuses_removed_block() {
    let device = xattr_fixture();
    let first_block;
    {
        let fs = Ext2FileSystem::new(device.clone()).unwrap();
        let node = xattr_node(&fs);
        node.set_xattr(b"user.serial", b"alpha", SetMode::Create)
            .unwrap();
        // On-disk ext2 assigns bit 3 to external attributes. Bit 2 denotes
        // a journal, and must stay clear on this non-journaled fixture.
        let disk_superblock=read_block(&device,1);
        let compat=u32::from_le_bytes(disk_superblock[92..96].try_into().unwrap());
        assert_eq!(compat & 0x0008,0x0008);
        assert_eq!(compat & 0x0004,0);
        assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"alpha");
        assert_eq!(
            node.set_xattr(b"user.serial", b"duplicate", SetMode::Create),
            Err(XattrError::Exists)
        );
        assert_eq!(
            node.set_xattr(b"user.missing", b"value", SetMode::Replace),
            Err(XattrError::Missing)
        );
        let inode = fs.read_inode(TARGET_INODE).unwrap();
        first_block = inode.get_file_acl();
        assert_ne!(first_block, 0);
        assert_eq!(inode.get_blocks(), 2);
        let on_disk = read_block(&device, first_block as u64);
        assert_eq!(&on_disk[0..4], &EXT2_XATTR_MAGIC.to_le_bytes());
        assert_eq!(&on_disk[4..8], &1_u32.to_le_bytes());
        assert_eq!(&on_disk[8..12], &1_u32.to_le_bytes());
        assert_eq!(on_disk[32], b"serial".len() as u8);
        assert_eq!(on_disk[33], EXT2_XATTR_INDEX_USER);
        assert_eq!(&on_disk[48..54], b"serial");
        let value_offset =
            u16::from_le_bytes(on_disk[34..36].try_into().unwrap()) as usize;
        assert_eq!(&on_disk[value_offset..value_offset + 5], b"alpha");
        assert_eq!(&on_disk[56..60], &[0; 4]);
        assert_ne!(
            u32::from_le_bytes(read_block(&device, 1)[92..96].try_into().unwrap())
                & EXT2_FEATURE_COMPAT_EXT_ATTR,
            0
        );
        assert!(bitmap_has(&device, BLOCK_BITMAP_BLOCK, first_block as usize - 1));
    }

    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"alpha");
    node.remove_xattr(b"user.serial").unwrap();
    assert_eq!(node.remove_xattr(b"user.serial"), Err(XattrError::Missing));
    assert_eq!(fs.read_inode(TARGET_INODE).unwrap().get_file_acl(), 0);
    assert_eq!(fs.read_inode(TARGET_INODE).unwrap().get_blocks(), 0);
    assert!(!bitmap_has(
        &device,
        BLOCK_BITMAP_BLOCK,
        first_block as usize - 1
    ));

    node.set_xattr(b"user.serial", b"beta", SetMode::Create)
        .unwrap();
    let reused = fs.read_inode(TARGET_INODE).unwrap().get_file_acl();
    assert_eq!(reused, first_block);
    drop(node);
    drop(fs);

    let remounted = Ext2FileSystem::new(device).unwrap();
    let node = xattr_node(&remounted);
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"beta");
    assert_eq!(node.list_xattrs().unwrap(), vec![b"user.serial".to_vec()]);
}

#[test_case]
fn ext2_xattr_shared_block_is_copied_and_inode_destroy_reclaims_owned_block() {
    let device = xattr_fixture();
    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    node.set_xattr(b"user.serial", b"old", SetMode::Create)
        .unwrap();
    let old_block = fs.read_inode(TARGET_INODE).unwrap().get_file_acl();
    let mut shared = read_block(&device, old_block as u64);
    shared[4..8].copy_from_slice(&2_u32.to_le_bytes());
    write_block(&device, old_block as u64, shared);
    fs.block_cache.write().remove(old_block as u64);

    node.set_xattr(b"user.serial", b"new", SetMode::Replace)
        .unwrap();
    let new_block = fs.read_inode(TARGET_INODE).unwrap().get_file_acl();
    assert_ne!(new_block, old_block);
    assert_eq!(u32::from_le_bytes(read_block(&device, old_block as u64)[4..8].try_into().unwrap()), 1);
    assert!(bitmap_has(&device, BLOCK_BITMAP_BLOCK, old_block as usize - 1));
    assert!(bitmap_has(&device, BLOCK_BITMAP_BLOCK, new_block as usize - 1));

    drop(node);
    fs.free_inode(TARGET_INODE).unwrap();
    assert!(!bitmap_has(&device, BLOCK_BITMAP_BLOCK, new_block as usize - 1));
    assert!(!bitmap_has(&device, INODE_BITMAP_BLOCK, (TARGET_INODE - 1) as usize));

    set_inode_allocated(&device, TARGET_INODE);
    let mut inode = Ext2Inode::empty();
    inode.mode = (EXT2_S_IFREG | 0o644).to_le();
    inode.links_count = 1_u16.to_le();
    fs.write_inode(TARGET_INODE, &inode).unwrap();
    let node = xattr_node(&fs);
    node.set_xattr(b"user.reused", b"ok", SetMode::Create)
        .unwrap();
    drop(node);
    drop(fs);

    let remounted = Ext2FileSystem::new(device).unwrap();
    assert_eq!(
        xattr_node(&remounted).get_xattr(b"user.reused").unwrap(),
        b"ok"
    );
}

#[test_case]
fn ext2_failed_created_inode_cleanup_reclaims_xattr_without_dir_underflow() {
    let device = xattr_fixture();
    let mut descriptors = read_block(&device, 2);
    descriptors[16..18].copy_from_slice(&1_u16.to_le_bytes());
    write_block(&device, 2, descriptors);
    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let mut inode = Ext2Inode::empty();
    inode.mode = (EXT2_S_IFDIR | 0o755).to_le();
    inode.links_count = 2_u16.to_le();
    fs.write_inode(TARGET_INODE, &inode).unwrap();
    let node = xattr_node(&fs);
    node.set_xattr(b"user.partial", b"label", SetMode::Create)
        .unwrap();
    let block = fs.read_inode(TARGET_INODE).unwrap().get_file_acl();
    drop(node);

    fs.discard_created_inode(TARGET_INODE).unwrap();
    assert!(!bitmap_has(&device, BLOCK_BITMAP_BLOCK, block as usize - 1));
    assert!(!bitmap_has(&device, INODE_BITMAP_BLOCK, (TARGET_INODE - 1) as usize));
    assert_eq!(
        u16::from_le_bytes(read_block(&device, 2)[16..18].try_into().unwrap()),
        1
    );
}

#[test_case]
fn ext2_xattr_namespaces_bounds_and_corruption_are_explicit() {
    assert_eq!(EXT2_XATTR_INDEX_USER, 1);
    assert_eq!(EXT2_XATTR_INDEX_POSIX_ACL_ACCESS, 2);
    assert_eq!(EXT2_XATTR_INDEX_POSIX_ACL_DEFAULT, 3);
    assert_eq!(EXT2_XATTR_INDEX_TRUSTED, 4);
    assert_eq!(EXT2_XATTR_INDEX_SYSTEM, 7);
    assert_eq!(EXT2_XATTR_INDEX_SECURITY, 6);
    assert_eq!(
        Ext2FileSystem::xattr_name_parts(b"user.name").unwrap(),
        (EXT2_XATTR_INDEX_USER, b"name".as_slice())
    );
    assert_eq!(
        Ext2FileSystem::xattr_name_parts(b"trusted.name").unwrap(),
        (EXT2_XATTR_INDEX_TRUSTED, b"name".as_slice())
    );
    assert_eq!(
        Ext2FileSystem::xattr_name_parts(b"security.name").unwrap(),
        (EXT2_XATTR_INDEX_SECURITY, b"name".as_slice())
    );
    assert_eq!(
        Ext2FileSystem::xattr_name_parts(b"system.name").unwrap(),
        (EXT2_XATTR_INDEX_SYSTEM, b"name".as_slice())
    );
    assert_eq!(
        Ext2FileSystem::xattr_name_parts(b"system.posix_acl_access").unwrap(),
        (EXT2_XATTR_INDEX_POSIX_ACL_ACCESS, b"".as_slice())
    );
    assert_eq!(
        Ext2FileSystem::xattr_name_parts(b"opaque.name").unwrap(),
        (0, b"opaque.name".as_slice())
    );

    let device = xattr_fixture();
    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    assert_eq!(
        node.set_xattr(b"user.too-large", &vec![0x5a; 2048], SetMode::Create),
        Err(XattrError::NoSpace)
    );
    assert_eq!(fs.read_inode(TARGET_INODE).unwrap().get_file_acl(), 0);

    node.set_xattr(b"user.serial", b"value", SetMode::Create)
        .unwrap();
    let block = fs.read_inode(TARGET_INODE).unwrap().get_file_acl();
    let mut corrupt = read_block(&device, block as u64);
    corrupt[0..4].fill(0);
    write_block(&device, block as u64, corrupt);
    fs.block_cache.write().remove(block as u64);
    assert_eq!(node.list_xattrs(), Err(XattrError::Io));
    assert_eq!(node.get_xattr(b"user.serial"), Err(XattrError::Io));
    assert!(fs.free_inode(TARGET_INODE).is_err());
    assert!(bitmap_has(&device, INODE_BITMAP_BLOCK, (TARGET_INODE - 1) as usize));
    assert!(bitmap_has(&device, BLOCK_BITMAP_BLOCK, block as usize - 1));
}

#[test_case]
fn ext2_native_label_metadata_is_disk_backed_bounded_and_returns_eio_on_corruption() {
    let device = xattr_fixture();
    let domain = [0x42; 16];
    let key = xattr::label_metadata_key(&domain);
    {
        let fs = Ext2FileSystem::new(device.clone()).unwrap();
        let node = xattr_node(&fs);
        assert_eq!(node.read_label_metadata(&domain).unwrap(), None);
        node.write_label_metadata(&domain, b"persistent-label").unwrap();
        node.set_xattr(b"user.visible", b"yes", SetMode::Create)
            .unwrap();
        assert_eq!(
            node.read_label_metadata(&domain).unwrap(),
            Some(b"persistent-label".to_vec())
        );
        assert_eq!(node.raw_get_xattr(&key).unwrap(), b"persistent-label");
        assert_eq!(node.list_xattrs().unwrap(), vec![b"user.visible".to_vec()]);
        assert_eq!(node.get_xattr(&key), Err(XattrError::PermissionDenied));
        assert_eq!(
            node.set_xattr(&key, b"public", SetMode::Upsert),
            Err(XattrError::PermissionDenied)
        );
        assert_eq!(node.remove_xattr(&key), Err(XattrError::PermissionDenied));
        assert_eq!(
            node.write_label_metadata(&domain, &vec![0; 513]).unwrap_err().kind,
            FileSystemErrorKind::ValueOverflow
        );
        node.write_label_metadata(&domain, b"").unwrap();
        assert_eq!(
            node.read_label_metadata(&domain).unwrap(),
            Some(Vec::new())
        );
        assert!(node.raw_get_xattr(&key).unwrap().is_empty());
        assert_eq!(node.list_xattrs().unwrap(), vec![b"user.visible".to_vec()]);
    }

    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    assert_eq!(
        node.read_label_metadata(&domain).unwrap(),
        Some(Vec::new())
    );
    let block = fs.read_inode(TARGET_INODE).unwrap().get_file_acl();
    let mut corrupt = read_block(&device, block as u64);
    corrupt[0..4].fill(0);
    write_block(&device, block as u64, corrupt);
    fs.block_cache.write().remove(block as u64);
    assert_eq!(
        node.read_label_metadata(&domain).unwrap_err().kind,
        FileSystemErrorKind::IoError
    );
}
