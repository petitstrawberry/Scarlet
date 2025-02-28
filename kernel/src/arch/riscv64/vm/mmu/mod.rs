// #[cfg(target_feature = "riscv_sv48")]
pub mod sv48;
// #[cfg(target_feature = "riscv_sv48")]
pub use sv48::*;

use crate::println;
use crate::print;

use super::alloc_virtual_address_space;
use super::get_page_table;
use super::get_root_page_table_idx;

pub fn mmu_init() {
    let asid = alloc_virtual_address_space(); /* Kernel ASID */
    let root_page_table_idx = get_root_page_table_idx(asid).unwrap();
    let root_page_table = get_page_table(root_page_table_idx).unwrap();
    root_page_table.map(0x80000000, 0x80000000, 0x1000);
    /* Enable MMU */
    root_page_table.switch(asid);
}