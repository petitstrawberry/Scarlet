//! Initramfs mounting functionality (VFS v2)
//!
//! This module provides functionality to mount the initial ramdisk (initramfs)
//! as the root filesystem during early kernel boot. The initramfs is loaded by
//! the bootloader and its location is passed to the kernel via the device tree.
//!
//! The module uses the existing CPIO filesystem driver to mount the initramfs
//! at the root ("/") mount point.

use core::ptr;
use alloc::string::ToString;
use alloc::sync::Arc;
use crate::device::fdt::FdtManager;
use crate::fs::VfsManager;
use crate::early_println;
use crate::fs::FileSystemError;
use crate::vm::vmem::MemoryArea;

static mut INITRAMFS_AREA: Option<MemoryArea> = None;

/// Relocate initramfs to heap memory
pub fn relocate_initramfs(usable_area: &mut MemoryArea) -> Result<(), &'static str> {
    early_println!("[InitRamFS] Relocating initramfs to {:#x}", usable_area.start as usize);
    let fdt_manager = FdtManager::get_manager();
    let original_area = fdt_manager.get_initramfs()
        .ok_or("Failed to get initramfs from device tree")?;
    let size = original_area.size();
    early_println!("[InitRamFS] Original initramfs at {:#x}, size: {} bytes", original_area.start, size);
    let new_ptr = usable_area.start as *mut u8;
    usable_area.start = new_ptr as usize + size;
    unsafe {
        ptr::copy_nonoverlapping(
            original_area.start as *const u8,
            new_ptr,
            size
        );
    }
    let new_area = MemoryArea::new(new_ptr as usize, (new_ptr as usize) + size - 1);
    early_println!("[InitRamFS] Relocated initramfs to {:#x}", new_area.start);
    unsafe { INITRAMFS_AREA = Some(new_area) };
    Ok(())
}

fn mount_initramfs(manager: &Arc<VfsManager>, initramfs: MemoryArea) -> Result<(), FileSystemError> {
    early_println!("[InitRamFS] Initializing initramfs");
    early_println!("[InitRamFS] Using initramfs at address: {:#x}, size: {} bytes", initramfs.start, initramfs.size());
    // CPIOイメージからファイルシステムを生成
    let cpio_data = unsafe {
        core::slice::from_raw_parts(initramfs.start as *const u8, initramfs.size())
    };
    let fs = crate::fs::vfs_v2::cpiofs::CpioFS::new("initramfs".to_string(), cpio_data)?;
    manager.mount(fs, "/", 0)?;
    early_println!("[InitRamFS] Successfully mounted initramfs at root directory");
    Ok(())
}

#[allow(static_mut_refs)]
pub fn init_initramfs(manager: &Arc<VfsManager>) {
    let initramfs_ptr = unsafe { INITRAMFS_AREA.as_ref().map(|area| area.start as *const u8).unwrap_or(core::ptr::null()) };
    if !initramfs_ptr.is_null() {
        let initramfs = unsafe { *INITRAMFS_AREA.as_ref().unwrap() };
        if let Err(e) = mount_initramfs(manager, initramfs.clone()) {
            early_println!("[InitRamFS] Warning: Could not mount initramfs: {:?}", e);
            return;
        }
    } else {
        early_println!("[InitRamFS] Warning: Initramfs relocation failed, cannot mount");
    }
}
