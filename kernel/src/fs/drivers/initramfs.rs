//! Initramfs mounting functionality
//!
//! This module provides functionality to mount the initial ramdisk (initramfs)
//! as the root filesystem during early kernel boot. The initramfs is loaded by
//! the bootloader and its location is passed to the kernel via the device tree.
//!
//! The module uses the existing CPIO filesystem driver to mount the initramfs
//! at the root ("/") mount point.

use core::ptr;

use alloc::string::ToString;

use crate::arch::get_cpu;
use crate::device::fdt::FdtManager;
use crate::sched::scheduler::get_scheduler;
use crate::task::elf_loader::load_elf_into_task;
use crate::task::new_user_task;
use crate::late_initcall;
use crate::early_println;
use crate::fs::{get_vfs_manager, File, FileSystemError};
use crate::vm::vmem::MemoryArea;

static mut INITRAMFS_AREA: Option<MemoryArea> = None;

/// Relocate initramfs to heap memory
///
/// This function copies the initramfs from the location provided by the bootloader
/// to a new location in kernel heap memory, so that it can be accessed after
/// virtual memory is enabled.
///
/// # Returns
/// Option<MemoryArea>: The memory area of the relocated initramfs if successful,
/// None otherwise.
pub fn relocate_initramfs(usable_area: &mut MemoryArea) -> Result<(), &'static str> {
    early_println!("[InitRamFS] Relocating initramfs to {:#x}", usable_area.start as usize);
    
    // Get the FDT manager
    let fdt_manager = FdtManager::get_manager();
    
    // Get the initramfs memory area from the device tree
    let original_area = fdt_manager.get_initramfs()
        .ok_or("Failed to get initramfs from device tree")?;
    
    let size = original_area.size();
    early_println!("[InitRamFS] Original initramfs at {:#x}, size: {} bytes", 
        original_area.start, size);
    
    let new_ptr = usable_area.start as *mut u8;
    usable_area.start = new_ptr as usize + size;

    // Copy the initramfs data
    unsafe {
        ptr::copy_nonoverlapping(
            original_area.start as *const u8,
            new_ptr,
            size
        );
    }
    
    // Create a new memory area for the relocated initramfs
    let new_area = MemoryArea::new(new_ptr as usize, (new_ptr as usize) + size - 1);
    early_println!("[InitRamFS] Relocated initramfs to {:#x}", new_area.start);
    
    unsafe { INITRAMFS_AREA = Some(new_area) };

    Ok(())
}

/// Mount the initramfs as the root filesystem
///
/// This function creates a CPIO filesystem from the initramfs memory area
/// and mounts it at the root ("/") mount point.
///
/// # Arguments
/// * `initramfs` - The memory area of the initramfs.
///
/// # Returns
/// Result<(), FileSystemError>: Ok if mounting was successful, Err otherwise.
fn mount_initramfs(initramfs: MemoryArea) -> Result<(), FileSystemError> {
    early_println!("[InitRamFS] Initializing initramfs");
    
    early_println!("[InitRamFS] Using initramfs at address: {:#x}, size: {} bytes", 
        initramfs.start, initramfs.size());

    // Get the VFS manager
    let vfs_manager = get_vfs_manager();
    
    // Create and register a CPIO filesystem from the initramfs memory area
    let fs_id = vfs_manager.create_and_register_memory_fs("cpiofs", &initramfs)?;
    
    // Mount the filesystem at the root directory
    match vfs_manager.mount(fs_id, "/") {
        Ok(_) => {
            early_println!("[InitRamFS] Successfully mounted initramfs at root directory");
            Ok(())
        },
        Err(e) => {
            early_println!("[InitRamFS] Failed to mount initramfs: {:?}", e);
            Err(e)
        }
    }
}

/// Late initialization of initramfs
///
/// This function is called after virtual memory is set up to mount
/// the initramfs as the root filesystem.
#[allow(static_mut_refs)]
fn late_init_initramfs() {
    let initramfs_ptr = unsafe { INITRAMFS_AREA.as_ref().map(|area| area.start as *const u8).unwrap_or(core::ptr::null()) };
    if !initramfs_ptr.is_null() {
        let initramfs = unsafe { *INITRAMFS_AREA.as_ref().unwrap() };
        
        // Mount the initramfs
        if let Err(e) = mount_initramfs(initramfs.clone()) {
            early_println!("[InitRamFS] Warning: Could not mount initramfs: {:?}", e);
            return;
        }
    } else {
        early_println!("[InitRamFS] Warning: Initramfs relocation failed, cannot mount");
    }
}

// Register the initramfs initialization functions to be called during boot
late_initcall!(late_init_initramfs);