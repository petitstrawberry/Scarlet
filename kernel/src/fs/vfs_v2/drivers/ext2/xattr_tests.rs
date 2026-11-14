use super::*;
use crate::fs::vfs_v2::core::FileSystemOperations;
use crate::fs::vfs_v2::xattr::{self, Error as XattrError, SetMode};

const TARGET_INODE: u32 = 2;
const INODE_TABLE_BLOCK: u64 = 5;
const BLOCK_BITMAP_BLOCK: u64 = 3;
const INODE_BITMAP_BLOCK: u64 = 4;
const LARGE_INODE_SIZE: usize = 256;
const EXTRA_ISIZE: usize = 32;
const IBODY_OFFSET: usize = EXT2_GOOD_OLD_INODE_SIZE + EXTRA_ISIZE;

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

fn large_inode_xattr_fixture() -> Arc<MockBlockDevice> {
    let device = xattr_fixture();
    let mut superblock = read_block(&device, 1);
    superblock[88..90].copy_from_slice(&(LARGE_INODE_SIZE as u16).to_le_bytes());
    write_block(&device, 1, superblock);
    device
}

fn target_inode_record(device: &MockBlockDevice) -> Vec<u8> {
    let table = read_block(device, INODE_TABLE_BLOCK);
    let start = (TARGET_INODE as usize - 1) * LARGE_INODE_SIZE;
    table[start..start + LARGE_INODE_SIZE].to_vec()
}

fn write_target_inode_record(device: &MockBlockDevice, record: &[u8]) {
    assert_eq!(record.len(), LARGE_INODE_SIZE);
    let mut table = read_block(device, INODE_TABLE_BLOCK);
    let start = (TARGET_INODE as usize - 1) * LARGE_INODE_SIZE;
    table[start..start + LARGE_INODE_SIZE].copy_from_slice(record);
    write_block(device, INODE_TABLE_BLOCK, table);
}

fn inline_fixture(records: &[Ext2XattrRecord]) -> Arc<MockBlockDevice> {
    let device = large_inode_xattr_fixture();
    let mut record = vec![0; LARGE_INODE_SIZE];
    let mut inode = Ext2Inode::empty();
    inode.mode = (EXT2_S_IFREG | 0o644).to_le();
    inode.links_count = 1_u16.to_le();
    let inode_bytes = unsafe {
        core::slice::from_raw_parts(
            &inode as *const Ext2Inode as *const u8,
            core::mem::size_of::<Ext2Inode>(),
        )
    };
    record[..inode_bytes.len()].copy_from_slice(inode_bytes);
    record[EXT2_INODE_EXTRA_ISIZE_OFFSET..EXT2_INODE_EXTRA_ISIZE_OFFSET + 2]
        .copy_from_slice(&(EXTRA_ISIZE as u16).to_le_bytes());
    // Unknown extra inode fields before the EA body must survive round trips.
    record[IBODY_OFFSET - 12..IBODY_OFFSET].fill(0x5a);

    if !records.is_empty() {
        let body_len = LARGE_INODE_SIZE - IBODY_OFFSET;
        record[IBODY_OFFSET..IBODY_OFFSET + 4].copy_from_slice(&EXT2_XATTR_MAGIC.to_le_bytes());
        let entries_start = IBODY_OFFSET + EXT2_XATTR_IBODY_HEADER_SIZE;
        let mut cursor = EXT2_XATTR_IBODY_HEADER_SIZE;
        let mut values_end = body_len - EXT2_XATTR_IBODY_HEADER_SIZE;
        let mut positions = Vec::with_capacity(records.len());
        let mut value_offsets = Vec::with_capacity(records.len());
        for item in records {
            positions.push(cursor);
            cursor += ext2_xattr_entry_len(item.name.len()).unwrap();
            let padded_value_len = ext2_xattr_value_len(item.value.len()).unwrap();
            values_end -= padded_value_len;
            value_offsets.push(values_end);
        }
        for ((item, position), value_offset) in records.iter().zip(positions).zip(value_offsets) {
            let entry = IBODY_OFFSET + position;
            let name_end = entry + EXT2_XATTR_ENTRY_SIZE + item.name.len();
            record[entry] = item.name.len() as u8;
            record[entry + 1] = item.name_index;
            record[entry + 2..entry + 4].copy_from_slice(&(value_offset as u16).to_le_bytes());
            record[entry + 4..entry + 8].copy_from_slice(&0_u32.to_le_bytes());
            record[entry + 8..entry + 12].copy_from_slice(&(item.value.len() as u32).to_le_bytes());
            record[entry + 12..entry + 16].copy_from_slice(&0_u32.to_le_bytes());
            record[entry + EXT2_XATTR_ENTRY_SIZE..name_end].copy_from_slice(&item.name);
            let value = entries_start + value_offset;
            record[value..value + item.value.len()].copy_from_slice(&item.value);
        }
    }
    write_target_inode_record(&device, &record);
    device
}

fn xattr_record(name_index: u8, name: &[u8], value: &[u8]) -> Ext2XattrRecord {
    Ext2XattrRecord {
        name_index,
        name: name.to_vec(),
        value: value.to_vec(),
    }
}

fn install_external_xattrs(fs: &Arc<Ext2FileSystem>, records: &[Ext2XattrRecord]) -> u32 {
    fs.ensure_xattr_feature().unwrap();
    let encoded = fs.encode_xattr_block(records).unwrap();
    let block = fs.allocate_block_locked().unwrap() as u32;
    fs.write_block_cached(u64::from(block), &encoded).unwrap();
    let mut inode = fs.read_inode(TARGET_INODE).unwrap();
    fs.add_inode_allocated_blocks(&mut inode, 1).unwrap();
    inode.set_file_acl(block);
    fs.write_inode(TARGET_INODE, &inode).unwrap();
    block
}

fn xattr_node(fs: &Arc<Ext2FileSystem>) -> Arc<Ext2Node> {
    let node = Arc::new(Ext2Node::new(TARGET_INODE, FileType::RegularFile, 1));
    node.set_filesystem(Arc::downgrade(
        &(fs.clone() as Arc<dyn FileSystemOperations>),
    ));
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
        let disk_superblock = read_block(&device, 1);
        let compat = u32::from_le_bytes(disk_superblock[92..96].try_into().unwrap());
        assert_eq!(compat & 0x0008, 0x0008);
        assert_eq!(compat & 0x0004, 0);
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
        let value_offset = u16::from_le_bytes(on_disk[34..36].try_into().unwrap()) as usize;
        assert_eq!(&on_disk[value_offset..value_offset + 5], b"alpha");
        assert_eq!(&on_disk[56..60], &[0; 4]);
        assert_ne!(
            u32::from_le_bytes(read_block(&device, 1)[92..96].try_into().unwrap())
                & EXT2_FEATURE_COMPAT_EXT_ATTR,
            0
        );
        assert!(bitmap_has(
            &device,
            BLOCK_BITMAP_BLOCK,
            first_block as usize - 1
        ));
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
fn ext2_128_byte_inode_keeps_ea_external_and_preserves_neighbor_record() {
    let device = xattr_fixture();
    let target_offset = (TARGET_INODE as usize - 1) * EXT2_GOOD_OLD_INODE_SIZE;
    let neighbor_offset = target_offset + EXT2_GOOD_OLD_INODE_SIZE;
    let mut table = read_block(&device, INODE_TABLE_BLOCK);
    table[neighbor_offset..neighbor_offset + EXT2_GOOD_OLD_INODE_SIZE].fill(0x5a);
    write_block(&device, INODE_TABLE_BLOCK, table);

    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    node.set_xattr(b"user.serial", b"0", SetMode::Create)
        .unwrap();
    assert_ne!(fs.read_inode(TARGET_INODE).unwrap().get_file_acl(), 0);
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"0");

    let body = fs.read_inode_xattr_body(TARGET_INODE).unwrap();
    assert_eq!(body.data.len(), EXT2_GOOD_OLD_INODE_SIZE);
    assert_eq!(body.body_offset, body.data.len());
    assert!(body.records.is_empty());
    let table = read_block(&device, INODE_TABLE_BLOCK);
    assert!(
        table[neighbor_offset..neighbor_offset + EXT2_GOOD_OLD_INODE_SIZE]
            .iter()
            .all(|byte| *byte == 0x5a)
    );
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
    assert_eq!(
        u32::from_le_bytes(
            read_block(&device, old_block as u64)[4..8]
                .try_into()
                .unwrap()
        ),
        1
    );
    assert!(bitmap_has(
        &device,
        BLOCK_BITMAP_BLOCK,
        old_block as usize - 1
    ));
    assert!(bitmap_has(
        &device,
        BLOCK_BITMAP_BLOCK,
        new_block as usize - 1
    ));

    drop(node);
    fs.free_inode(TARGET_INODE).unwrap();
    assert!(!bitmap_has(
        &device,
        BLOCK_BITMAP_BLOCK,
        new_block as usize - 1
    ));
    assert!(!bitmap_has(
        &device,
        INODE_BITMAP_BLOCK,
        (TARGET_INODE - 1) as usize
    ));

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
    assert!(!bitmap_has(
        &device,
        INODE_BITMAP_BLOCK,
        (TARGET_INODE - 1) as usize
    ));
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
    assert!(bitmap_has(
        &device,
        INODE_BITMAP_BLOCK,
        (TARGET_INODE - 1) as usize
    ));
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
        node.write_label_metadata(&domain, b"persistent-label")
            .unwrap();
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
            node.write_label_metadata(&domain, &vec![0; 513])
                .unwrap_err()
                .kind,
            FileSystemErrorKind::ValueOverflow
        );
        node.write_label_metadata(&domain, b"").unwrap();
        assert_eq!(node.read_label_metadata(&domain).unwrap(), Some(Vec::new()));
        assert!(node.raw_get_xattr(&key).unwrap().is_empty());
        assert_eq!(node.list_xattrs().unwrap(), vec![b"user.visible".to_vec()]);
    }

    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    assert_eq!(node.read_label_metadata(&domain).unwrap(), Some(Vec::new()));
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

#[test_case]
fn ext2_inline_and_external_xattrs_roundtrip_without_losing_inode_metadata() {
    let device = inline_fixture(&[
        xattr_record(EXT2_XATTR_INDEX_USER, b"inline", b"initial"),
        xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"0"),
    ]);
    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    let untouched_extra_fields =
        target_inode_record(&device)[IBODY_OFFSET - 12..IBODY_OFFSET].to_vec();

    assert_eq!(node.get_xattr(b"user.inline").unwrap(), b"initial");
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"0");
    assert_eq!(
        node.list_xattrs().unwrap(),
        vec![b"user.inline".to_vec(), b"user.serial".to_vec()]
    );

    let large_value = vec![0x41; 80];
    node.set_xattr(b"user.inline", &large_value, SetMode::Replace)
        .unwrap();
    let external_block = fs.read_inode(TARGET_INODE).unwrap().get_file_acl();
    assert_ne!(external_block, 0);
    let body = fs.read_inode_xattr_body(TARGET_INODE).unwrap();
    assert_eq!(
        body.records,
        vec![xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"0")]
    );
    assert_eq!(
        fs.read_xattr_block(&fs.read_inode(TARGET_INODE).unwrap())
            .unwrap()
            .unwrap()
            .records,
        vec![xattr_record(EXT2_XATTR_INDEX_USER, b"inline", &large_value)]
    );

    node.set_xattr(b"user.serial", b"1", SetMode::Replace)
        .unwrap();
    assert_eq!(
        fs.read_inode(TARGET_INODE).unwrap().get_file_acl(),
        external_block
    );
    assert_eq!(
        fs.read_inode_xattr_body(TARGET_INODE).unwrap().records,
        vec![xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"1")]
    );

    node.set_xattr(b"user.extra", b"inline", SetMode::Create)
        .unwrap();
    fs.set_ext2_xattr(
        TARGET_INODE,
        b"trusted.scarlet.shield.fixture",
        b"SLB1u:object_r:system_data_file:s0",
        SetMode::Create,
    )
    .unwrap();
    assert_eq!(
        fs.read_inode(TARGET_INODE).unwrap().get_file_acl(),
        external_block
    );
    let body = fs.read_inode_xattr_body(TARGET_INODE).unwrap();
    assert_eq!(
        body.records,
        vec![
            xattr_record(EXT2_XATTR_INDEX_USER, b"extra", b"inline"),
            xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"1"),
        ]
    );
    assert_eq!(
        target_inode_record(&device)[IBODY_OFFSET - 12..IBODY_OFFSET].to_vec(),
        untouched_extra_fields
    );

    drop(node);
    drop(fs);
    let remounted = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&remounted);
    assert_eq!(node.get_xattr(b"user.inline").unwrap(), large_value);
    assert_eq!(node.get_xattr(b"user.extra").unwrap(), b"inline");
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"1");
    node.remove_xattr(b"user.inline").unwrap();
    remounted
        .remove_ext2_xattr(TARGET_INODE, b"trusted.scarlet.shield.fixture")
        .unwrap();
    assert_eq!(
        remounted.read_inode(TARGET_INODE).unwrap().get_file_acl(),
        0
    );
    assert!(!bitmap_has(
        &device,
        BLOCK_BITMAP_BLOCK,
        external_block as usize - 1
    ));
    assert_eq!(node.get_xattr(b"user.extra").unwrap(), b"inline");
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"1");
    node.set_xattr(b"user.x", b"i", SetMode::Create).unwrap();
    assert_eq!(
        remounted.read_inode(TARGET_INODE).unwrap().get_file_acl(),
        0
    );
    assert_eq!(node.get_xattr(b"user.x").unwrap(), b"i");
    assert_eq!(
        target_inode_record(&device)[IBODY_OFFSET - 12..IBODY_OFFSET].to_vec(),
        untouched_extra_fields
    );
    let final_record = target_inode_record(&device);
    assert_eq!(
        u16::from_le_bytes(
            final_record[EXT2_INODE_EXTRA_ISIZE_OFFSET..EXT2_INODE_EXTRA_ISIZE_OFFSET + 2]
                .try_into()
                .unwrap()
        ),
        EXTRA_ISIZE as u16
    );
    assert_eq!(
        remounted
            .read_inode_xattr_body(TARGET_INODE)
            .unwrap()
            .body_offset,
        IBODY_OFFSET
    );
}

#[test_case]
fn ext2_mixed_storage_duplicate_is_deduplicated_and_conflicts_are_rejected() {
    let device = inline_fixture(&[xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"0")]);
    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    install_external_xattrs(&fs, &[xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"0")]);
    let node = xattr_node(&fs);
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"0");
    assert_eq!(node.list_xattrs().unwrap(), vec![b"user.serial".to_vec()]);

    node.set_xattr(b"user.other", b"x", SetMode::Create)
        .unwrap();
    assert_eq!(
        fs.read_inode_xattr_body(TARGET_INODE).unwrap().records,
        vec![xattr_record(EXT2_XATTR_INDEX_USER, b"other", b"x")]
    );
    assert_eq!(
        node.list_xattrs().unwrap(),
        vec![b"user.serial".to_vec(), b"user.other".to_vec()]
    );

    node.set_xattr(b"user.serial", b"1", SetMode::Replace)
        .unwrap();
    assert_eq!(
        fs.read_inode_xattr_body(TARGET_INODE).unwrap().records,
        vec![xattr_record(EXT2_XATTR_INDEX_USER, b"other", b"x")]
    );
    assert_eq!(
        fs.read_xattr_block(&fs.read_inode(TARGET_INODE).unwrap())
            .unwrap()
            .unwrap()
            .records,
        vec![xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"1")]
    );
    assert_eq!(
        node.list_xattrs().unwrap(),
        vec![b"user.serial".to_vec(), b"user.other".to_vec()]
    );

    let conflict_device = inline_fixture(&[xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"0")]);
    let conflict_fs = Ext2FileSystem::new(conflict_device).unwrap();
    install_external_xattrs(
        &conflict_fs,
        &[xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"1")],
    );
    let conflict_node = xattr_node(&conflict_fs);
    assert_eq!(conflict_node.get_xattr(b"user.serial"), Err(XattrError::Io));
    assert_eq!(conflict_node.list_xattrs(), Err(XattrError::Io));
    assert_eq!(
        conflict_node.set_xattr(b"user.serial", b"2", SetMode::Replace),
        Err(XattrError::Io)
    );
    assert_eq!(
        conflict_fs
            .read_inode_xattr_body(TARGET_INODE)
            .unwrap()
            .records,
        vec![xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"0")]
    );
}

#[test_case]
fn ext2_inode_body_xattr_rejects_out_of_bounds_extra_isize() {
    let device = inline_fixture(&[xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"0")]);
    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    let node = xattr_node(&fs);
    let mut record = target_inode_record(&device);
    record[EXT2_INODE_EXTRA_ISIZE_OFFSET..EXT2_INODE_EXTRA_ISIZE_OFFSET + 2]
        .copy_from_slice(&34_u16.to_le_bytes());
    write_target_inode_record(&device, &record);
    fs.block_cache.write().remove(INODE_TABLE_BLOCK);
    assert_eq!(node.list_xattrs(), Err(XattrError::Io));

    record[EXT2_INODE_EXTRA_ISIZE_OFFSET..EXT2_INODE_EXTRA_ISIZE_OFFSET + 2]
        .copy_from_slice(&240_u16.to_le_bytes());
    write_target_inode_record(&device, &record);
    fs.block_cache.write().remove(INODE_TABLE_BLOCK);
    assert_eq!(node.get_xattr(b"user.serial"), Err(XattrError::Io));
}

#[test_case]
fn ext2_inode_body_accepts_unsorted_entries_without_changing_values() {
    let device = inline_fixture(&[
        xattr_record(EXT2_XATTR_INDEX_USER, b"inline", b"first"),
        xattr_record(EXT2_XATTR_INDEX_USER, b"serial", b"second"),
    ]);
    let mut record = target_inode_record(&device);
    let start = IBODY_OFFSET + EXT2_XATTR_IBODY_HEADER_SIZE;
    let width = ext2_xattr_entry_len(6).unwrap();
    let first = record[start..start + width].to_vec();
    record.copy_within(start + width..start + 2 * width, start);
    record[start + width..start + 2 * width].copy_from_slice(&first);
    write_target_inode_record(&device, &record);
    let fs = Ext2FileSystem::new(device).unwrap();
    let node = xattr_node(&fs);
    assert_eq!(node.get_xattr(b"user.inline").unwrap(), b"first");
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"second");
    node.set_xattr(b"user.serial", b"updated", SetMode::Replace)
        .unwrap();
    assert_eq!(node.get_xattr(b"user.inline").unwrap(), b"first");
    assert_eq!(node.get_xattr(b"user.serial").unwrap(), b"updated");
}

#[test_case]
fn ext2_new_inode_identity_does_not_inherit_old_inline_attributes() {
    let device = inline_fixture(&[xattr_record(EXT2_XATTR_INDEX_USER, b"secret", b"old")]);
    let fs = Ext2FileSystem::new(device.clone()).unwrap();
    assert_eq!(xattr_node(&fs).get_xattr(b"user.secret").unwrap(), b"old");
    fs.free_inode(TARGET_INODE).unwrap();
    set_inode_allocated(&device, TARGET_INODE);
    let mut inode = Ext2Inode::empty();
    inode.mode = (EXT2_S_IFREG | 0o644).to_le();
    inode.links_count = 1_u16.to_le();
    fs.write_new_inode(TARGET_INODE, &inode).unwrap();
    let node = xattr_node(&fs);
    assert!(node.list_xattrs().unwrap().is_empty());
    assert_eq!(node.get_xattr(b"user.secret"), Err(XattrError::Missing));
    assert!(
        target_inode_record(&device)[EXT2_GOOD_OLD_INODE_SIZE..]
            .iter()
            .all(|byte| *byte == 0)
    );
    node.set_xattr(b"user.new", b"new", SetMode::Create)
        .unwrap();
    assert_eq!(node.get_xattr(b"user.new").unwrap(), b"new");
}

#[test_case]
fn ext2_inode_body_accepts_a_four_byte_extra_field_area() {
    let device = inline_fixture(&[]);
    let mut record = target_inode_record(&device);
    record[EXT2_GOOD_OLD_INODE_SIZE..].fill(0);
    record[EXT2_INODE_EXTRA_ISIZE_OFFSET..EXT2_INODE_EXTRA_ISIZE_OFFSET + 2]
        .copy_from_slice(&4_u16.to_le_bytes());
    write_target_inode_record(&device, &record);
    let fs = Ext2FileSystem::new(device).unwrap();
    let node = xattr_node(&fs);
    node.set_xattr(b"user.small", b"value", SetMode::Create)
        .unwrap();
    assert_eq!(node.get_xattr(b"user.small").unwrap(), b"value");
    assert_eq!(fs.read_inode(TARGET_INODE).unwrap().get_file_acl(), 0);
    assert_eq!(
        fs.read_inode_xattr_body(TARGET_INODE).unwrap().body_offset,
        132
    );
}
