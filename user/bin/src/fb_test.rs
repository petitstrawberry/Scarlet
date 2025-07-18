//! Framebuffer test application
//! 
//! This application demonstrates framebuffer control operations
//! using the new framebuffer library.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use framebuffer::Framebuffer;

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("Framebuffer Control Test using new framebuffer library");
    
    // Open framebuffer device
    let mut framebuffer = match Framebuffer::open("/dev/fb0") {
        Ok(device) => {
            println!("Successfully opened /dev/fb0");
            device
        }
        Err(e) => {
            println!("Failed to open /dev/fb0: {:?}", e);
            return 1;
        }
    };
    
    // Get screen information
    let var_info = match framebuffer.get_var_screen_info() {
        Ok(var_info) => {
            println!("Variable Screen Info:");
            println!("  Resolution: {}x{}", var_info.xres, var_info.yres);
            println!("  Virtual Resolution: {}x{}", var_info.xres_virtual, var_info.yres_virtual);
            println!("  Bits per pixel: {}", var_info.bits_per_pixel);
            println!("  Red channel: offset={}, length={}", var_info.red.offset, var_info.red.length);
            println!("  Green channel: offset={}, length={}", var_info.green.offset, var_info.green.length);
            println!("  Blue channel: offset={}, length={}", var_info.blue.offset, var_info.blue.length);
            var_info
        }
        Err(e) => {
            println!("Failed to get variable screen info: {:?}", e);
            return 1;
        }
    };
    
    let _fix_info = match framebuffer.get_fix_screen_info() {
        Ok(fix_info) => {
            println!("Fixed Screen Info:");
            let id_str = core::str::from_utf8(&fix_info.id).unwrap_or("(invalid)");
            println!("  ID: {}", id_str);
            println!("  Memory start: 0x{:x}", fix_info.smem_start);
            println!("  Memory length: {} bytes", fix_info.smem_len);
            println!("  Line length: {} bytes", fix_info.line_length);
            println!("  Type: {}", fix_info.type_);
            println!("  Visual: {}", fix_info.visual);
            fix_info
        }
        Err(e) => {
            println!("Failed to get fixed screen info: {:?}", e);
            return 1;
        }
    };

    // Test 1: Fill screen with red
    println!("Test 1: Filling screen with red...");
    if let Err(e) = framebuffer.fill_screen([0, 0, 255, 255]) {
        println!("Failed to fill screen with red: {:?}", e);
        return 1;
    }
    
    // Flush to display
    if let Err(e) = framebuffer.flush() {
        println!("Failed to flush framebuffer: {:?}", e);
        return 1;
    }
    println!("Red fill completed and flushed");
    
    // Wait a bit (simple loop)
    for _ in 0..10000000 { 
        unsafe { core::arch::asm!("nop"); } 
    }
    
    // Test 2: Fill screen with green
    println!("Test 2: Filling screen with green...");
    if let Err(e) = framebuffer.fill_screen([0, 255, 0, 255]) {
        println!("Failed to fill screen with green: {:?}", e);
        return 1;
    }
    
    if let Err(e) = framebuffer.flush() {
        println!("Failed to flush framebuffer: {:?}", e);
        return 1;
    }
    println!("Green fill completed and flushed");
    
    // Wait a bit
    for _ in 0..10000000 { 
        unsafe { core::arch::asm!("nop"); } 
    }
    
    // Test 3: Fill screen with blue
    println!("Test 3: Filling screen with blue...");
    if let Err(e) = framebuffer.fill_screen([255, 0, 0, 255]) {
        println!("Failed to fill screen with blue: {:?}", e);
        return 1;
    }
    
    if let Err(e) = framebuffer.flush() {
        println!("Failed to flush framebuffer: {:?}", e);
        return 1;
    }
    println!("Blue fill completed and flushed");
    
    // Wait a bit
    for _ in 0..10000000 { 
        unsafe { core::arch::asm!("nop"); } 
    }
    
    // Test 4: Draw test pattern
    println!("Test 4: Drawing test pattern...");
    if let Err(e) = framebuffer.draw_test_pattern() {
        println!("Failed to draw test pattern: {:?}", e);
        return 1;
    }
    
    if let Err(e) = framebuffer.flush() {
        println!("Failed to flush framebuffer: {:?}", e);
        return 1;
    }
    println!("Test pattern completed and flushed");
    
    // Wait a bit
    for _ in 0..10000000 { 
        unsafe { core::arch::asm!("nop"); } 
    }
    
    // Test 5: Draw gradient
    println!("Test 5: Drawing gradient...");
    if let Err(e) = framebuffer.draw_gradient() {
        println!("Failed to draw gradient: {:?}", e);
        return 1;
    }
    
    if let Err(e) = framebuffer.flush() {
        println!("Failed to flush framebuffer: {:?}", e);
        return 1;
    }
    println!("Gradient completed and flushed");
    
    // Test 6: Draw some rectangles
    println!("Test 6: Drawing rectangles...");
    let _width = var_info.xres;
    let _height = var_info.yres;
    
    // Clear to black first
    if let Err(e) = framebuffer.fill_screen([0, 0, 0, 255]) {
        println!("Failed to clear screen: {:?}", e);
        return 1;
    }
    
    // Draw colorful rectangles
    if let Err(e) = framebuffer.fill_rect(50, 50, 100, 100, [0, 0, 255, 255]) {
        println!("Failed to draw red rectangle: {:?}", e);
        return 1;
    }
    
    if let Err(e) = framebuffer.fill_rect(200, 50, 100, 100, [0, 255, 0, 255]) {
        println!("Failed to draw green rectangle: {:?}", e);
        return 1;
    }
    
    if let Err(e) = framebuffer.fill_rect(350, 50, 100, 100, [255, 0, 0, 255]) {
        println!("Failed to draw blue rectangle: {:?}", e);
        return 1;
    }
    
    if let Err(e) = framebuffer.flush() {
        println!("Failed to flush framebuffer: {:?}", e);
        return 1;
    }
    println!("Rectangles completed and flushed");
    
    println!("All framebuffer tests completed successfully!");
    0
}