//! Exercise real PMM allocations, backing padding and segmented CPU views.
use super::*;
use crate::device::gpu::{GPU_IMAGE_FORMAT_BGRA8_UNORM, GpuBackendImageLayout, GpuImageCreateInfo};

const UAT_PAGE_SIZE: usize = 16 * 1024;

fn layout(size: usize, alignment: usize) -> GpuBackendImageLayout {
    let create = GpuImageCreateInfo::new(GPU_IMAGE_FORMAT_BGRA8_UNORM, 0, 1, 1);
    let mut layout = GpuBackendImageLayout::tight_32bpp(create).unwrap();
    layout.total_size = size as u64;
    layout.alignment = alignment as u64;
    layout
}

fn assert_extents(backing: &GpuPrivateImageBacking, alignment: usize) {
    let mut length = 0;
    for chunk in &backing.chunks {
        assert_eq!(chunk.as_paddr() % alignment as u64, 0);
        assert_eq!(chunk.len() * PAGE_SIZE % alignment, 0);
        length += chunk.len() * PAGE_SIZE;
    }
    assert_eq!(length, backing.allocation_size);
    assert_eq!(backing.physical_segments.len(), backing.chunks.len());
    for (chunk, segment) in backing.chunks.iter().zip(backing.physical_segments.iter()) {
        assert_eq!(segment.physical_addr(), chunk.as_paddr());
        assert_eq!(segment.length(), chunk.len() * PAGE_SIZE);
    }
}

#[test_case]
fn image_backing_aligns_and_zeroes_the_complete_uat_page() {
    let layout = layout(PAGE_SIZE + 1, UAT_PAGE_SIZE);
    let backing = GpuPrivateImageBacking::new(layout, false).unwrap();
    assert_eq!(layout.total_size, (PAGE_SIZE + 1) as u64);
    assert_eq!(backing.allocation_size, UAT_PAGE_SIZE);
    assert_extents(&backing, UAT_PAGE_SIZE);
    backing
        .for_each_range(0, backing.allocation_size, |address, length| {
            // SAFETY: the backing owns this range and no backend or DMA user has
            // received it. The allocator must zero padding as well as image data.
            let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
            assert!(bytes.iter().all(|byte| *byte == 0));
            Ok(())
        })
        .unwrap();
}

#[test_case]
fn image_backing_bounds_physical_chunks_and_slices_without_staging() {
    let chunk_size = GpuPrivateImageBacking::MAX_CHUNK_PAGES * PAGE_SIZE;
    let backing =
        GpuPrivateImageBacking::new(layout(2 * chunk_size + PAGE_SIZE + 1, UAT_PAGE_SIZE), true)
            .unwrap();
    assert!(backing.chunks.len() >= 3);
    assert_eq!(backing.allocation_size, 2 * chunk_size + UAT_PAGE_SIZE);
    assert_extents(&backing, UAT_PAGE_SIZE);
    assert!(
        backing
            .chunks
            .iter()
            .all(|chunk| chunk.len() * PAGE_SIZE <= chunk_size)
    );
    // The physical buddy reservation must also fit that budget after the
    // absolute-alignment prefix, not just the requested/visible chunk length.
    for chunk in &backing.chunks {
        let with_prefix = chunk.len() + UAT_PAGE_SIZE / PAGE_SIZE - 1;
        assert!(with_prefix.next_power_of_two() <= GpuPrivateImageBacking::MAX_CHUNK_PAGES);
    }

    // Choose the actual first boundary, so this also works under fragmentation.
    let boundary = backing.chunks[0].len() * PAGE_SIZE;
    let mut calls = 0;
    backing
        .for_each_range(boundary - 8, 16, |address, length| {
            assert_eq!(length, 8);
            let expected = if calls == 0 {
                backing.chunks[0].as_vaddr() + boundary - 8
            } else {
                backing.chunks[1].as_vaddr()
            };
            assert_eq!(address, expected);
            // SAFETY: the image backing is exclusive, and the supplied range stays
            // live for this call. Write through the view into the original pages.
            unsafe { core::ptr::write_bytes(address as *mut u8, 0x51 + calls, length) };
            calls += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(calls, 2);
    // SAFETY: same live backing as above, with no concurrent CPU/device user.
    unsafe {
        assert_eq!(
            (backing.chunks[0].as_vaddr() as *const u8)
                .add(boundary - 1)
                .read(),
            0x51
        );
        assert_eq!((backing.chunks[1].as_vaddr() as *const u8).read(), 0x52);
    }
    assert!(
        backing
            .for_each_range(backing.allocation_size - 1, 2, |_, _| Ok(()))
            .is_err()
    );
}

#[test_case]
fn image_backing_accepts_a_granule_larger_than_the_preferred_chunk() {
    let alignment = 2 * GpuPrivateImageBacking::MAX_CHUNK_PAGES * PAGE_SIZE;
    let backing = GpuPrivateImageBacking::new(layout(4, alignment), true).unwrap();
    assert_eq!(backing.allocation_size, alignment);
    assert_extents(&backing, alignment);
}

#[test_case]
fn image_backing_rejects_invalid_layout_and_rounding_overflow() {
    assert!(GpuPrivateImageBacking::new(layout(4, 3), true).is_err());
    assert!(GpuPrivateImageBacking::new(layout(usize::MAX, UAT_PAGE_SIZE), true).is_err());
    assert!(GpuPrivateImageBacking::new(layout(0, UAT_PAGE_SIZE), true).is_err());
}
