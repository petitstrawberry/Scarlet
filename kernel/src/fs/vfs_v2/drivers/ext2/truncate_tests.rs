use super::*;
use crate::environment::PAGE_SIZE;
use crate::fs::vfs_v2::cache::CacheId;
use crate::mem::page_cache::PageCacheManager;

fn configure_free_blocks(fs: &Ext2FileSystem, first: u32, count: u16) {
    let mut descriptors = vec![0; 1024];
    descriptors[0..4].copy_from_slice(&3_u32.to_le_bytes());
    descriptors[4..8].copy_from_slice(&4_u32.to_le_bytes());
    descriptors[8..12].copy_from_slice(&5_u32.to_le_bytes());
    descriptors[12..14].copy_from_slice(&count.to_le_bytes());
    fs.write_block_cached(2, &descriptors).unwrap();
    let mut bitmap = vec![0xff; 1024];
    for block in first..first + u32::from(count) {
        let bit = (block - 1) as usize;
        bitmap[bit / 8] &= !(1 << (bit % 8));
    }
    fs.write_block_cached(3, &bitmap).unwrap();
    fs.write_block_cached(4, &vec![0xff; 1024]).unwrap();
    let mut superblock = fs.read_block_cached(1).unwrap();
    superblock[12..16].copy_from_slice(&u32::from(count).to_le_bytes());
    fs.write_block_cached(1, &superblock).unwrap();
}

#[test_case]
fn ext2_sparse_growth_needs_no_free_blocks_and_clears_raw_eof_tail() {
    let (_device, fs, node) = create_writeback_test_file();
    configure_free_blocks(&fs, 600, 0);
    let before_bitmap = fs.read_block_cached(3).unwrap();
    fs.write_block_cached(300, &vec![b'A'; 1024]).unwrap();
    let file = fs.open(&node, 0).unwrap();
    let duplicate = file.clone();
    let other = fs.open(&node, 0).unwrap();
    file.seek_signed(123, 0).unwrap();
    other.seek_signed(456, 0).unwrap();
    let cache_id = CacheId::new((fs.fs_id().get() << 32) | 11);
    for size in [32 * 1024 * 1024, u32::MAX as u64] {
        file.truncate(size).unwrap();
        assert_eq!(file.metadata().unwrap().size as u64, size);
        assert_eq!(duplicate.seek_signed(0, 1).unwrap(), 123);
        assert_eq!(other.seek_signed(0, 1).unwrap(), 456);
        let inode = fs.read_inode(11).unwrap();
        assert_eq!(inode.get_blocks(), 2);
        let blocks = inode.block;
        assert_eq!(blocks[0], 300);
        assert!(blocks[1..].iter().all(|block| *block == 0));
        let mut bytes = [1; 8];
        assert_eq!(file.read_at(size - 8, &mut bytes).unwrap(), 8);
        assert_eq!(bytes, [0; 8]);
    }
    let disk = fs.read_block_cached(300).unwrap();
    assert!(disk[..64].iter().all(|byte| *byte == b'A'));
    assert!(disk[64..].iter().all(|byte| *byte == 0));
    assert_eq!(fs.read_block_cached(3).unwrap(), before_bitmap);
    file.truncate(0).unwrap();
    file.truncate(u32::MAX as u64).unwrap();
    drop(duplicate);
    drop(other);
    drop(file);
    PageCacheManager::global().invalidate(cache_id);
    let reopened = fs.open(&node, 0).unwrap();
    let mut bytes = [1; 128];
    assert_eq!(reopened.read_at(0, &mut bytes).unwrap(), bytes.len());
    assert_eq!(bytes, [0; 128]);
    assert_eq!(fs.read_inode(11).unwrap().get_blocks(), 2);
    assert_eq!(fs.read_block_cached(3).unwrap(), before_bitmap);
    drop(reopened);
    PageCacheManager::global().invalidate(cache_id);
}

#[test_case]
fn ext2_clean_uncached_partial_truncate_zeroes_disk_before_regrowth() {
    let (_device, fs, node) = create_writeback_test_file();
    fs.write_block_cached(300, &vec![b'B'; 1024]).unwrap();
    let file = fs.open(&node, 0).unwrap();
    let cache_id = CacheId::new((fs.fs_id().get() << 32) | 11);
    // No page has been loaded or dirtied when the boundary is shortened.
    file.truncate(13).unwrap();
    file.truncate(1024).unwrap();
    drop(file);
    PageCacheManager::global().invalidate(cache_id);
    let reopened = fs.open(&node, 0).unwrap();
    let mut bytes = [1; 1024];
    assert_eq!(reopened.read_at(0, &mut bytes).unwrap(), bytes.len());
    assert!(bytes[..13].iter().all(|byte| *byte == b'B'));
    assert!(bytes[13..].iter().all(|byte| *byte == 0));
    drop(reopened);
    PageCacheManager::global().invalidate(cache_id);
}

#[test_case]
fn ext2_truncate_flushes_multiple_dirty_batches_with_pins_and_counts_pointer_tables() {
    let (_device, fs, node) = create_writeback_test_file();
    configure_free_blocks(&fs, 600, 200);
    let file = fs.open(&node, 0).unwrap();
    let cache_id = CacheId::new((fs.fs_id().get() << 32) | 11);
    const PAGES: usize = 96;
    for page in 0..PAGES {
        assert_eq!(
            file.write_at((page * PAGE_SIZE + 11) as u64, &[page as u8 + 1])
                .unwrap(),
            1
        );
    }
    let pinned_first = PageCacheManager::global().try_pin(cache_id, 0).unwrap();
    let pinned_second_batch = PageCacheManager::global().try_pin(cache_id, 64).unwrap();
    let new_size = ((PAGES - 1) * PAGE_SIZE + 12) as u64;
    // Growth still flushes retained dirty data without allocating the gaps.
    file.truncate(new_size + 1).unwrap();
    let inode = fs.read_inode(11).unwrap();
    // 96 data blocks, one single-indirect table, a double root and its leaf.
    assert_eq!(inode.get_blocks(), (PAGES as u32 + 3) * 2);
    let blocks = inode.block;
    assert_ne!(blocks[12], 0);
    assert_ne!(blocks[13], 0);
    assert_eq!(blocks[14], 0);
    for page in [0, 2, 3, 66, 67, 64, 95] {
        let logical = (page * PAGE_SIZE / 1024) as u64;
        let block = fs.get_inode_block(&inode, logical).unwrap();
        assert_ne!(block, 0);
        assert_eq!(fs.read_block_cached(block).unwrap()[11], page as u8 + 1);
        assert_eq!(fs.get_inode_block(&inode, logical + 1).unwrap(), 0);
    }
    unsafe {
        assert_eq!(
            *(crate::vm::addr::phys_to_virt(pinned_first.paddr()) as *const u8).add(11),
            1
        );
        assert_eq!(
            *(crate::vm::addr::phys_to_virt(pinned_second_batch.paddr()) as *const u8).add(11),
            65
        );
    }
    drop(pinned_first);
    drop(pinned_second_batch);
    let allocation_before_shrink = fs.read_block_cached(3).unwrap();
    file.truncate(5).unwrap();
    assert_eq!(
        fs.read_inode(11).unwrap().get_blocks(),
        (PAGES as u32 + 3) * 2
    );
    file.truncate(new_size + 1).unwrap();
    assert_eq!(fs.read_block_cached(3).unwrap(), allocation_before_shrink);
    drop(file);
    PageCacheManager::global().invalidate(cache_id);
    let reopened = fs.open(&node, 0).unwrap();
    for page in 0..PAGES {
        let mut bytes = [1; 16];
        assert_eq!(
            reopened
                .read_at((page * PAGE_SIZE) as u64, &mut bytes)
                .unwrap(),
            if page == PAGES - 1 { 13 } else { 16 }
        );
        assert!(
            bytes[..if page == PAGES - 1 { 13 } else { 16 }]
                .iter()
                .all(|byte| *byte == 0)
        );
    }
    drop(reopened);
    PageCacheManager::global().invalidate(cache_id);
    // Even EOF=0 must reclaim every retained data and indirect block.
    let file = fs.open(&node, 0).unwrap();
    file.truncate(0).unwrap();
    drop(file);
    PageCacheManager::global().invalidate(cache_id);
    let mut allocated = Vec::new();
    fs.walk_inode_allocated_blocks(&fs.read_inode(11).unwrap(), &mut |_, block, _| {
        allocated.push(block);
        Ok(())
    })
    .unwrap();
    assert_eq!(allocated.len(), PAGES + 3);
    fs.free_inode(11).unwrap();
    let bitmap = fs.read_block_cached(3).unwrap();
    for block in allocated {
        let bit = (block - 1) as usize;
        assert_eq!(bitmap[bit / 8] & (1 << (bit % 8)), 0);
    }
    assert_eq!(fs.read_inode(11).unwrap().get_mode(), 0);
}

#[test_case]
fn ext2_unsupported_triple_allocation_is_rejected_before_earlier_page_allocation() {
    let (_device, fs, node) = create_writeback_test_file();
    configure_free_blocks(&fs, 600, 8);
    let bitmap = fs.read_block_cached(3).unwrap();
    let file = fs.open(&node, 0).unwrap();
    let cache_id = CacheId::new((fs.fs_id().get() << 32) | 11);
    let triple_start = (12 + 256 + 256 * 256) * 1024_u64;
    file.write_at(PAGE_SIZE as u64, b"early page").unwrap();
    file.write_at(triple_start, b"unsupported page").unwrap();
    assert!(file.sync().is_err());
    assert_eq!(fs.read_block_cached(3).unwrap(), bitmap);
    assert_eq!(fs.read_inode(11).unwrap().get_blocks(), 2);
    // Discard unpersisted pages so Drop does not retry unsupported writeback.
    file.truncate(0).unwrap();
    drop(file);
    PageCacheManager::global().invalidate(cache_id);
}
