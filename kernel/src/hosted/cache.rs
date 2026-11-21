//! Memory ordering for coherent hosted buffers and Darwin instruction-cache
//! synchronization.

use core::sync::atomic::{Ordering, fence};

/// Scarlet and the host adapters exchange bytes in coherent normal memory.
/// The host manages device DMA. Keep the ordering edge at the existing
/// architecture boundary without issuing bare-metal cache maintenance.
#[inline]
pub fn order_host_memory(_address: usize, len: usize) {
    if len != 0 {
        fence(Ordering::SeqCst);
    }
}

pub use order_host_memory as clean_dcache_to_poc_range;
pub use order_host_memory as clean_dcache_to_pou_range;
pub use order_host_memory as clean_invalidate_dcache_to_poc_range;
pub use order_host_memory as invalidate_dcache_to_poc_range;

pub fn sync_icache_for_execution(start_vaddr: usize, len: usize) {
    unsafe extern "C" {
        fn sys_icache_invalidate(start: *const core::ffi::c_void, len: usize);
    }

    if len != 0 {
        // SAFETY: Callers supply the still-mapped kernel alias they just wrote.
        unsafe { sys_icache_invalidate(start_vaddr as *const core::ffi::c_void, len) };
    }
}
