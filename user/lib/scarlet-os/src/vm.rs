//! Process address-placement policy, independent of ABI and execution Environment.
use crate::handle::{HandleError, HandleResult};
use scarlet_sys::{Syscall, syscall1};
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct Layout {
    pub native_bits: u32,
    pub low_bits: u32,
    pub mmap_base: u64,
}
pub fn configure_layout(native_bits: u8, low_bits: u8) -> HandleResult<()> {
    // SAFETY: the kernel validates entropy ranges and native management authority.
    if unsafe {
        syscall1(
            Syscall::VmLayoutConfigure,
            usize::from(native_bits) | (usize::from(low_bits) << 8),
        )
    } == 0
    {
        Ok(())
    } else {
        Err(HandleError::InvalidParameter)
    }
}
pub fn layout() -> HandleResult<Layout> {
    let mut layout = Layout::default();
    // SAFETY: fixed 16-byte output is exclusive for the duration of this copy.
    if unsafe { syscall1(Syscall::VmLayoutQuery, &mut layout as *mut Layout as usize) } == 0 {
        Ok(layout)
    } else {
        Err(HandleError::InvalidParameter)
    }
}
