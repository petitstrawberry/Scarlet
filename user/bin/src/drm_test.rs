//! DRM test application
//! 
//! This application demonstrates DRM control operations
//! using the Linux DRM ioctl interface.

#![no_std]
#![no_main]

extern crate scarlet_std as std;
extern crate alloc;

use std::println;
use std::fs::File;
use std::handle::capability::memory_mapping::{mmap, munmap, prot, flags};
use std::thread;
use std::time::Duration;

// DRM Ioctl numbers
const DRM_IOCTL_MODE_CREATE_DUMB: u32 = 0xC02064B2;
const DRM_IOCTL_MODE_MAP_DUMB: u32 = 0xC01064B3;
const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = 0xC00464B4;
const DRM_IOCTL_MODE_PAGE_FLIP: u32 = 0xC01064B0;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct DrmModeCreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct DrmModeMapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct DrmModeDestroyDumb {
    handle: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct DrmModePageFlip {
    crtc_id: u32,
    fb_id: u32,
    flags: u32,
    reserved: u32,
    user_data: u64,
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("DRM Control Test");
    
    // Open DRM device
    // Note: GraphicsManager registers it as "card0" if it's the first one
    let path = "/dev/card0";
    let file = match File::open(path) {
        Ok(f) => {
            println!("Successfully opened {}", path);
            f
        }
        Err(e) => {
            println!("Failed to open {}: {:?}", path, e);
            return 1;
        }
    };
    
    // 1. Create Dumb Buffer
    // We'll try to create a buffer that matches a common screen size
    // In a real app, we would use GET_RESOURCES/GET_CRTC to find the right size
    let width = 1024;
    let height = 768;
    let bpp = 32;
    
    let mut create_dumb = DrmModeCreateDumb {
        width,
        height,
        bpp,
        flags: 0,
        ..Default::default()
    };
    
    println!("Creating dumb buffer {}x{} {}bpp...", width, height, bpp);
    if let Err(e) = file.as_handle().control(
        DRM_IOCTL_MODE_CREATE_DUMB,
        &mut create_dumb as *mut _ as usize
    ) {
        println!("Failed to create dumb buffer: {:?}", e);
        return 1;
    }
    
    println!("Dumb buffer created: handle={}, size={}, pitch={}", 
             create_dumb.handle, create_dumb.size, create_dumb.pitch);
             
    // 2. Map Dumb Buffer
    let mut map_dumb = DrmModeMapDumb {
        handle: create_dumb.handle,
        ..Default::default()
    };
    
    println!("Mapping dumb buffer...");
    if let Err(e) = file.as_handle().control(
        DRM_IOCTL_MODE_MAP_DUMB,
        &mut map_dumb as *mut _ as usize
    ) {
        println!("Failed to map dumb buffer: {:?}", e);
        return 1;
    }
    
    println!("Dumb buffer map offset: 0x{:x}", map_dumb.offset);
    
    // 3. mmap the buffer
    // Note: In Scarlet's current DRM implementation, the offset returned IS the physical address
    // and we use it as the offset for mmap.
    let mapped_addr = match mmap(
        file.as_handle().as_raw() as u32,
        0,
        create_dumb.size as usize,
        prot::READ | prot::WRITE,
        flags::SHARED,
        map_dumb.offset as usize // Use the offset returned by DRM
    ) {
        Ok(addr) => addr,
        Err(e) => {
            println!("mmap failed: {:?}", e);
            return 1;
        }
    };
    
    println!("Buffer mapped at virtual address: 0x{:x}", mapped_addr);
    
    let buffer_slice = unsafe {
        core::slice::from_raw_parts_mut(mapped_addr as *mut u8, create_dumb.size as usize)
    };
    
    // Helper to fill buffer with color
    let mut fill_color = |r: u8, g: u8, b: u8| {
        for i in 0..(width * height) as usize {
            let offset = i * 4;
            if offset + 4 <= buffer_slice.len() {
                // BGRA format usually
                buffer_slice[offset] = b;
                buffer_slice[offset + 1] = g;
                buffer_slice[offset + 2] = r;
                buffer_slice[offset + 3] = 255;
            }
        }
        
        // Perform page flip to update screen
        let mut page_flip = DrmModePageFlip {
            crtc_id: 1, // Assuming CRTC 1
            fb_id: create_dumb.handle,
            flags: 0,
            reserved: 0,
            user_data: 0,
        };
        
        if let Err(e) = file.as_handle().control(
            DRM_IOCTL_MODE_PAGE_FLIP,
            &mut page_flip as *mut _ as usize
        ) {
            println!("Failed to page flip: {:?}", e);
        }
    };
    
    // Test Sequence
    println!("Starting visual test sequence...");
    
    println!("1. Red Screen");
    fill_color(255, 0, 0);
    thread::sleep(Duration::from_secs(2));
    
    println!("2. Green Screen");
    fill_color(0, 255, 0);
    thread::sleep(Duration::from_secs(2));
    
    println!("3. Blue Screen");
    fill_color(0, 0, 255);
    thread::sleep(Duration::from_secs(2));
    
    println!("4. White Screen");
    fill_color(255, 255, 255);
    thread::sleep(Duration::from_secs(2));
    
    println!("5. Gradient");
    for y in 0..height {
        for x in 0..width {
            let offset = ((y * width + x) * 4) as usize;
            if offset + 4 <= buffer_slice.len() {
                let r = (x * 255 / width) as u8;
                let g = (y * 255 / height) as u8;
                let b = 255 - r;
                
                buffer_slice[offset] = b;
                buffer_slice[offset + 1] = g;
                buffer_slice[offset + 2] = r;
                buffer_slice[offset + 3] = 255;
            }
        }
    }
    
    // Perform page flip for gradient
    let mut page_flip = DrmModePageFlip {
        crtc_id: 1,
        fb_id: create_dumb.handle,
        flags: 0,
        reserved: 0,
        user_data: 0,
    };
    
    if let Err(e) = file.as_handle().control(
        DRM_IOCTL_MODE_PAGE_FLIP,
        &mut page_flip as *mut _ as usize
    ) {
        println!("Failed to page flip: {:?}", e);
    }
    
    thread::sleep(Duration::from_secs(2));
    
    // Cleanup
    println!("Cleaning up...");
    let _ = munmap(mapped_addr, create_dumb.size as usize);
    
    let mut destroy_dumb = DrmModeDestroyDumb {
        handle: create_dumb.handle,
    };
    
    if let Err(e) = file.as_handle().control(
        DRM_IOCTL_MODE_DESTROY_DUMB,
        &mut destroy_dumb as *mut _ as usize
    ) {
        println!("Failed to destroy dumb buffer: {:?}", e);
    }
    
    println!("Test completed successfully!");
    0
}
