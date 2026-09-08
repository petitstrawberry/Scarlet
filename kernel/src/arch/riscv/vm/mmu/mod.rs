//! RISC-V stage-1 paging geometry. Page-table algorithms are shared across XLEN.

#[cfg(target_pointer_width = "32")]
mod geometry {
    pub const MAX_PAGING_LEVEL: usize = 1;
    pub const INDEX_BITS: usize = 10;
    pub const PPN_BITS: usize = 22;
    pub const ASID_BITS: usize = 9;
    pub const SATP_MODE: usize = 1;
    pub const SATP_MODE_SHIFT: usize = 31;
    pub const SATP_PPN_BITS: usize = 22;
    pub const fn is_canonical(_address: usize) -> bool {
        true
    }
}

#[cfg(target_pointer_width = "64")]
mod geometry {
    pub const MAX_PAGING_LEVEL: usize = 3;
    pub const INDEX_BITS: usize = 9;
    pub const PPN_BITS: usize = 44;
    pub const ASID_BITS: usize = 16;
    pub const SATP_MODE: usize = 9;
    pub const SATP_MODE_SHIFT: usize = 60;
    pub const SATP_PPN_BITS: usize = 44;
    pub const fn is_canonical(address: usize) -> bool {
        ((address as isize) << 16 >> 16) as usize == address
    }
}

pub use geometry::*;
pub const TABLE_ENTRIES: usize = 1 << INDEX_BITS;
mod page_table;
pub use page_table::*;

const _: () = assert!(core::mem::size_of::<PageTable>() == crate::environment::PAGE_SIZE);
const _: () = assert!(core::mem::size_of::<PageTableEntry>() == core::mem::size_of::<usize>());
