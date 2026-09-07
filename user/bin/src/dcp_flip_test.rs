//! DCP immutable scanout page-flip diagnostic.
//!
//! This utility initializes the two direct scanout buffers with distinct solid
//! colors, then alternates them without modifying either buffer again.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use framebuffer::DisplaySurface;
use std::println;
use std::time::Duration;
use std::vec::Vec;

const RED_BGRA: [u8; 4] = [0, 0, 255, 255];
const GREEN_BGRA: [u8; 4] = [0, 255, 0, 255];
const FLIP_INTERVAL: Duration = Duration::from_secs(2);

fn read_first_pixel(address: usize) -> [u8; 4] {
    // SAFETY: `address` is returned by the live DisplaySurface mmap and covers
    // at least one four-byte BGRA pixel. This diagnostic owns the mappings and
    // does not write either scanout while these volatile reads are performed.
    unsafe { core::ptr::read_volatile(address as *const [u8; 4]) }
}

fn fill_current_scanout(
    display: &mut DisplaySurface,
    width: u32,
    height: u32,
    color: [u8; 4],
) -> Result<(), &'static str> {
    let stride = (width as usize)
        .checked_mul(4)
        .ok_or("scanout row size overflow")?;
    let buffer_size = stride
        .checked_mul(height as usize)
        .ok_or("scanout buffer size overflow")?;
    let mut pixels = Vec::new();
    pixels.resize(buffer_size, 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }

    display
        .write_bgra_strided(0, 0, width, height, &pixels, stride)
        .map_err(|_| "failed to fill current scanout")
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("DCP immutable scanout flip diagnostic");
    println!("Stop SWS before running this program.");

    let mut display = match DisplaySurface::open_primary() {
        Ok(display) => display,
        Err(error) => {
            println!("failed to open /dev/display0: {:?}", error);
            return 1;
        }
    };

    if display.swapchain_buffer_count() != 2 {
        println!(
            "expected exactly 2 scanout buffers, found {}",
            display.swapchain_buffer_count()
        );
        return 1;
    }

    let info = match display.get_info() {
        Ok(info) => info,
        Err(error) => {
            println!("failed to query display information: {:?}", error);
            return 1;
        }
    };
    println!(
        "display={}x{} stride={} format={} buffers=2",
        info.width, info.height, info.stride, info.format
    );

    let green_index = match display.draw_buffer_index() {
        Some(index) => index,
        None => {
            println!("green scanout has no draw-buffer index");
            return 1;
        }
    };
    let (green_address, _) = match display.get_mapping_info() {
        Some(mapping) => mapping,
        None => {
            println!("scanout 1 is not mapped");
            return 1;
        }
    };
    println!(
        "green scanout {} mapping: 0x{:x}",
        green_index, green_address
    );

    println!("initializing scanout {} with solid green", green_index);
    if let Err(error) = fill_current_scanout(&mut display, info.width, info.height, GREEN_BGRA) {
        println!("{}", error);
        return 1;
    }
    if let Err(error) = display.present() {
        println!(
            "failed to present green scanout {}: {:?}",
            green_index, error
        );
        return 1;
    }
    println!(
        "green scanout {} presented; readback={:02x?}; holding for 2 seconds",
        green_index,
        read_first_pixel(green_address)
    );
    std::thread::sleep(FLIP_INTERVAL);

    let red_index = match display.draw_buffer_index() {
        Some(index) => index,
        None => {
            println!("red scanout has no draw-buffer index");
            return 1;
        }
    };
    let (red_address, _) = match display.get_mapping_info() {
        Some(mapping) => mapping,
        None => {
            println!("scanout 0 is not mapped");
            return 1;
        }
    };
    println!("red scanout {} mapping: 0x{:x}", red_index, red_address);

    println!("initializing scanout {} with solid red", red_index);
    if let Err(error) = fill_current_scanout(&mut display, info.width, info.height, RED_BGRA) {
        println!("{}", error);
        return 1;
    }
    if let Err(error) = display.present() {
        println!("failed to present red scanout {}: {:?}", red_index, error);
        return 1;
    }

    println!(
        "readback after initialization: scanout {}={:02x?}, scanout {}={:02x?}",
        green_index,
        read_first_pixel(green_address),
        red_index,
        read_first_pixel(red_address)
    );

    println!("initialization complete; scanout buffers are now immutable");
    println!("expected sequence every 2 seconds: green, red, green, red, ...");

    let mut flip = 0u64;
    loop {
        std::thread::sleep(FLIP_INTERVAL);
        let expected = if flip & 1 == 0 { "green" } else { "red" };
        if let Err(error) = display.present() {
            println!("flip {} ({}) failed: {:?}", flip, expected, error);
            return 1;
        }
        println!(
            "flip {} complete; expected {}; scanout {}={:02x?}, scanout {}={:02x?}",
            flip,
            expected,
            green_index,
            read_first_pixel(green_address),
            red_index,
            read_first_pixel(red_address)
        );
        flip = flip.wrapping_add(1);
    }
}
