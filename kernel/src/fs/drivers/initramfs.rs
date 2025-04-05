//! Initramfs mounting functionality
//!
//! This module provides functionality to mount the initial ramdisk (initramfs)
//! as the root filesystem during early kernel boot. The initramfs is loaded by
//! the bootloader and its location is passed to the kernel via the device tree.
//!
//! The module uses the existing CPIO filesystem driver to mount the initramfs
//! at the root ("/") mount point.

use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use alloc::boxed::Box;

use crate::device::fdt::FdtManager;
use crate::{early_initcall, late_initcall};
use crate::{early_print, early_println};
use crate::fs::{get_vfs_manager, FileSystemError};
use crate::mem::kmalloc;
use crate::vm::vmem::MemoryArea;

static INITRAMFS_AREA: AtomicPtr<MemoryArea> = AtomicPtr::new(core::ptr::null_mut());

/// Relocate initramfs to heap memory
///
/// This function copies the initramfs from the location provided by the bootloader
/// to a new location in kernel heap memory, so that it can be accessed after
/// virtual memory is enabled.
///
/// # Returns
/// Option<MemoryArea>: The memory area of the relocated initramfs if successful,
/// None otherwise.
fn relocate_initramfs() -> Option<MemoryArea> {
    early_println!("[InitRamFS] Relocating initramfs to heap memory");
    
    // Get the FDT manager
    let fdt_manager = FdtManager::get_manager();
    
    // Get the initramfs memory area from the device tree
    let original_area = fdt_manager.get_initramfs()?;
    
    let size = original_area.size();
    early_println!("[InitRamFS] Original initramfs at {:#x}, size: {} bytes", 
        original_area.start, size);
    
    // Allocate memory for the initramfs
    let new_ptr = kmalloc(size);
    if new_ptr.is_null() {
        early_println!("[InitRamFS] Failed to allocate memory for initramfs");
        return None;
    }
    
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
    
    Some(new_area)
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

/// Early initialization of initramfs
///
/// This function is called during early kernel initialization to relocate
/// the initramfs to heap memory.
fn early_init_initramfs() {
    early_println!("[InitRamFS] Early initialization of initramfs");
    if INITRAMFS_AREA.load(Ordering::SeqCst).is_null() {
        if let Some(initramfs) = relocate_initramfs() {
            INITRAMFS_AREA.store(Box::into_raw(Box::new(initramfs)), Ordering::SeqCst);
        } else {
            early_println!("[InitRamFS] Warning: Failed to relocate initramfs");
        }
    }
}

/// Late initialization of initramfs
///
/// This function is called after virtual memory is set up to mount
/// the initramfs as the root filesystem.
fn late_init_initramfs() {
    let initramfs_ptr = INITRAMFS_AREA.load(Ordering::SeqCst);
    if !initramfs_ptr.is_null() {
        let initramfs = unsafe { &*initramfs_ptr };
        
        // Mount the initramfs
        if let Err(e) = mount_initramfs(initramfs.clone()) {
            early_println!("[InitRamFS] Warning: Could not mount initramfs: {:?}", e);
        } else {
            // Successfully mounted, free the memory
            early_println!("[InitRamFS] Mounted successfully, freeing initramfs memory");
            crate::mem::kfree(initramfs.start as *mut u8, initramfs.size());
            
            // Set the pointer to null to indicate it's been freed
            INITRAMFS_AREA.store(core::ptr::null_mut(), Ordering::SeqCst);
        }
    } else {
        early_println!("[InitRamFS] Warning: Initramfs relocation failed, cannot mount");
    }
}

// Register the initramfs initialization functions to be called during boot
early_initcall!(early_init_initramfs);
late_initcall!(late_init_initramfs);