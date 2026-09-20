//! Owned ADMA2 storage shared by SDHCI platform bindings.
//!
//! A retained, page-aligned bounce buffer avoids both per-command allocation
//! and cache-line aliases with arbitrary caller buffers. DMA mappings outlive
//! transfers and are dropped before their physical backing.

use crate::{
    arch,
    device::iommu::{DmaContext, DmaMapping, IommuMapFlags},
    environment::PAGE_SIZE,
    mem::page::ContiguousPages,
    vm::addr::PhysAddr,
};

pub(super) const MAX_TRANSFER: usize = 64 * 1024;
const SEGMENT_SIZE: usize = 32 * 1024;

pub(super) struct Adma2 {
    table_mapping: DmaMapping,
    buffer_mapping: DmaMapping,
    table: ContiguousPages,
    buffer: ContiguousPages,
    address_64: bool,
    descriptor_size: usize,
}

impl Adma2 {
    pub(super) fn new(
        context: &DmaContext,
        address_64: bool,
        padded_64: bool,
    ) -> Result<Self, &'static str> {
        let granule = context.mapping_granule();
        let table = ContiguousPages::new_aligned(granule / PAGE_SIZE, granule)
            .ok_or("SDHCI: ADMA table allocation failed")?;
        let buffer_len = MAX_TRANSFER.div_ceil(granule) * granule;
        let buffer = ContiguousPages::new_aligned(buffer_len / PAGE_SIZE, granule)
            .ok_or("SDHCI: ADMA buffer allocation failed")?;
        let table_mapping = context
            .map_phys_owned(
                PhysAddr::new(table.as_paddr()),
                granule,
                IommuMapFlags::READ,
            )
            .map_err(|_| "SDHCI: ADMA table mapping failed")?;
        let buffer_mapping = context
            .map_phys_owned(
                PhysAddr::new(buffer.as_paddr()),
                buffer_len,
                IommuMapFlags::READ | IommuMapFlags::WRITE,
            )
            .map_err(|_| "SDHCI: ADMA buffer mapping failed")?;
        if !address_64
            && (table_mapping.dma_addr().as_u64() + granule as u64 - 1 > u32::MAX as u64
                || buffer_mapping.dma_addr().as_u64() + buffer_len as u64 - 1 > u32::MAX as u64)
        {
            return Err("SDHCI: DMA allocations exceed the 32-bit address aperture");
        }
        Ok(Self {
            table_mapping,
            buffer_mapping,
            table,
            buffer,
            address_64,
            descriptor_size: if !address_64 {
                8
            } else if padded_64 {
                16
            } else {
                12
            },
        })
    }

    pub(super) fn host_control(&self) -> u8 {
        if self.address_64 { 3 << 3 } else { 2 << 3 }
    }

    pub(super) fn table_address(&self) -> u64 {
        self.table_mapping.dma_addr().as_u64()
    }

    pub(super) fn prepare(&mut self, len: usize, write_data: Option<&[u8]>) {
        debug_assert!(len != 0 && len <= MAX_TRANSFER);
        let buffer = self.buffer.as_vaddr();
        if let Some(data) = write_data {
            // SAFETY: caller data is independent of our private, retained DMA
            // allocation; send_command holds exclusive host ownership.
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buffer as *mut u8, len) };
            arch::clean_dcache_to_poc_range(buffer, len);
        } else {
            arch::clean_invalidate_dcache_to_poc_range(buffer, len);
        }

        let count = len.div_ceil(SEGMENT_SIZE);
        // SAFETY: at most two descriptors, in our exclusively owned page.
        let table = unsafe {
            core::slice::from_raw_parts_mut(
                self.table.as_vaddr() as *mut u8,
                count * self.descriptor_size,
            )
        };
        table.fill(0);
        for (index, descriptor) in table.chunks_exact_mut(self.descriptor_size).enumerate() {
            let offset = index * SEGMENT_SIZE;
            let bytes = (len - offset).min(SEGMENT_SIZE);
            // Never encode a zero length (64 KiB): Tegra210 cannot handle it.
            // END belongs on the last transfer, not on an extra NOP descriptor.
            let flags: u16 = 0x21 | if index + 1 == count { 2 } else { 0 };
            let address = self.buffer_mapping.dma_addr().as_u64() + offset as u64;
            descriptor[..2].copy_from_slice(&flags.to_le_bytes());
            descriptor[2..4].copy_from_slice(&(bytes as u16).to_le_bytes());
            descriptor[4..8].copy_from_slice(&(address as u32).to_le_bytes());
            if self.address_64 {
                descriptor[8..12].copy_from_slice(&((address >> 32) as u32).to_le_bytes());
            }
        }
        arch::clean_dcache_to_poc_range(self.table.as_vaddr(), table.len());
        arch::io_mb();
    }

    pub(super) fn finish_read(&self, data: &mut [u8]) {
        // TRANSFER_COMPLETE must be observed before invalidating/copying data.
        arch::io_mb();
        arch::invalidate_dcache_to_poc_range(self.buffer.as_vaddr(), data.len());
        // SAFETY: DMA is retired and destination cannot alias the private buffer.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.buffer.as_vaddr() as *const u8,
                data.as_mut_ptr(),
                data.len(),
            );
        }
    }
}
