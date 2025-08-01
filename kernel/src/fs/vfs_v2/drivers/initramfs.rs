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
    
    // Validate parameters before proceeding
    if size == 0 || size > 0x10000000 {
        return Err("Invalid initramfs size");
    }
    if original_area.start == 0 {
        return Err("Invalid initramfs source address");
    }
    
    // Ensure proper 8-byte alignment for destination
    let raw_ptr = usable_area.start as *mut u8;
    let aligned_ptr = ((raw_ptr as usize + 7) & !7) as *mut u8;
    let aligned_addr = aligned_ptr as usize;
    
    early_println!("[InitRamFS] Copying from {:#x} to {:#x} (aligned), size: {} bytes", 
                   original_area.start, aligned_addr, size);
    
    // Validate destination memory bounds
    if aligned_addr + size > usable_area.end {
        return Err("Insufficient memory for initramfs");
    }
    
    // Create the new memory area BEFORE the copy operation
    let new_area = MemoryArea::new(aligned_addr, aligned_addr + size - 1);
    
    // Perform the copy with explicit memory barriers
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    
    // Use a safer approach: copy in smaller chunks to avoid stack issues
    let chunk_size = 4096; // 4KB chunks
    let mut src_addr = original_area.start as *const u8;
    let mut dst_addr = aligned_ptr;
    let mut remaining = size;
    
    unsafe {
        while remaining > 0 {
            let copy_size = if remaining > chunk_size { chunk_size } else { remaining };
            ptr::copy_nonoverlapping(src_addr, dst_addr, copy_size);
            
            src_addr = src_addr.add(copy_size);
            dst_addr = dst_addr.add(copy_size);
            remaining -= copy_size;
            
            // Add memory barrier between chunks
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }
    
    // Update usable_area start AFTER copying, with alignment
    usable_area.start = (aligned_addr + size + 7) & !7;
    early_println!("[InitRamFS] Relocated initramfs to {:#x}, next usable: {:#x}", 
                   new_area.start, usable_area.start);
    unsafe { INITRAMFS_AREA = Some(new_area) };
    Ok(())
}

fn mount_initramfs(manager: &Arc<VfsManager>, initramfs: MemoryArea) -> Result<(), FileSystemError> {
    early_println!("[InitRamFS] Initializing initramfs");
    early_println!("[InitRamFS] Using initramfs at address: {:#x}, size: {} bytes", initramfs.start, initramfs.size());
    // Generate file system from CPIO image
    let cpio_data = unsafe {
        core::slice::from_raw_parts(initramfs.start as *const u8, initramfs.size())
    };
    let fs = crate::fs::vfs_v2::drivers::cpiofs::CpioFS::new("initramfs".to_string(), cpio_data)?;
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
