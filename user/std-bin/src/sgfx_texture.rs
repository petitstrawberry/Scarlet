//! Standalone sampled-texture and alpha-composition demo.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;
use std::vec::Vec;

use framebuffer::DisplaySurface;
use sgfx::{Color, CompositionPass, Device, PixelRect, SgfxImagePresentExt, SourceAlpha};

const MAX_TEXTURE_SIZE: u32 = 256;
const MARGIN: u32 = 16;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

fn checkerboard_bgra(size: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    pixels.resize(size as usize * size as usize * 4, 0);
    for y in 0..size {
        for x in 0..size {
            let offset = (y as usize * size as usize + x as usize) * 4;
            let checker = ((x / 16) + (y / 16)) % 2 == 0;
            pixels[offset] = 255u8.saturating_sub((x * 255 / size) as u8);
            pixels[offset + 1] = (y * 255 / size) as u8;
            pixels[offset + 2] = (x * 255 / size) as u8;
            pixels[offset + 3] = if checker { 224 } else { 64 };
        }
    }
    pixels
}

fn animated_patch_bgra(size: u32, frame: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    pixels.resize(size as usize * size as usize * 4, 0);
    let phase = (frame % 120) as u8;
    let alpha = if phase < 60 {
        32u8.saturating_add(phase.saturating_mul(3))
    } else {
        32u8.saturating_add((119 - phase).saturating_mul(3))
    };
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[32, 224, 255, alpha]);
    }
    pixels
}

fn fail(message: &str, error: impl core::fmt::Debug) -> ExitCode {
    println!("sgfx_texture: {}: {:?}", message, error);
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let display = match DisplaySurface::open_primary() {
        Ok(display) => display,
        Err(error) => return fail("failed to open primary display", error),
    };
    let display_info = match display.get_info() {
        Ok(info) => info,
        Err(error) => return fail("failed to query display", error),
    };
    if display_info.width <= MARGIN * 3 || display_info.height <= MARGIN * 2 {
        println!("sgfx_texture: display is too small");
        return ExitCode::from(1);
    }

    let device = match Device::open("/dev/gpu0") {
        Ok(device) => device,
        Err(error) => return fail("failed to open GPU", error),
    };
    let capabilities = device.capabilities();
    if !capabilities.supports_rendering()
        || !capabilities.supports_presentation()
        || !capabilities.supports_image_upload()
    {
        println!("sgfx_texture: texture composition is unsupported");
        return ExitCode::from(1);
    }

    let context = match device.create_context() {
        Ok(context) => context,
        Err(error) => return fail("failed to create context", error),
    };
    let image = match context.create_image(display_info.width, display_info.height) {
        Ok(image) => image,
        Err(error) => return fail("failed to create render target", error),
    };
    let available_width = (image.width() - MARGIN * 3) / 2;
    let available_height = image.height() - MARGIN * 2;
    let texture_size = MAX_TEXTURE_SIZE.min(available_width).min(available_height);
    if texture_size == 0 {
        println!("sgfx_texture: no room for texture panels");
        return ExitCode::from(1);
    }
    let texture = match context.create_sampled_bgra_texture(texture_size, texture_size) {
        Ok(texture) => texture,
        Err(error) => return fail("failed to create sampled texture", error),
    };
    let initial_pixels = checkerboard_bgra(texture_size);
    if let Err(error) = context.upload_texture_bgra(
        &texture,
        &initial_pixels,
        texture_size * 4,
        PixelRect::new(0, 0, texture_size, texture_size),
    ) {
        return fail("failed to upload initial texture", error);
    }
    let queue = match context.create_queue() {
        Ok(queue) => queue,
        Err(error) => return fail("failed to create queue", error),
    };

    let top = (image.height() - texture_size) / 2;
    let left_panel = PixelRect::new(MARGIN, top, texture_size, texture_size);
    let right_panel = PixelRect::new(MARGIN * 2 + texture_size, top, texture_size, texture_size);
    let source = PixelRect::new(0, 0, texture_size, texture_size);
    let patch_size = 32.min(texture_size);
    let patch_x = (texture_size - patch_size) / 2;
    let patch_y = (texture_size - patch_size) / 2;

    println!(
        "sgfx_texture: {}x{} texture; left respects source alpha, right ignores it",
        texture_size, texture_size
    );

    let mut frame = 0u32;
    loop {
        let patch = animated_patch_bgra(patch_size, frame);
        if let Err(error) = context.upload_texture_bgra(
            &texture,
            &patch,
            patch_size * 4,
            PixelRect::new(patch_x, patch_y, patch_size, patch_size),
        ) {
            return fail("failed to upload texture damage", error);
        }

        let mut composition = match CompositionPass::new(&image, Color::rgba(0.08, 0.1, 0.14, 1.0))
        {
            Ok(composition) => composition,
            Err(error) => return fail("failed to begin composition", error),
        };
        if let Err(error) =
            composition.draw_solid_rect(left_panel, Color::rgba(0.15, 0.25, 0.85, 1.0), None)
        {
            return fail("failed to draw left panel", error);
        }
        if let Err(error) = composition.draw_textured_rect(
            &texture,
            left_panel,
            source,
            1.0,
            SourceAlpha::Respect,
            None,
        ) {
            return fail("failed to draw alpha texture", error);
        }
        if let Err(error) =
            composition.draw_solid_rect(right_panel, Color::rgba(0.15, 0.75, 0.25, 1.0), None)
        {
            return fail("failed to draw right panel", error);
        }
        if let Err(error) = composition.draw_textured_rect(
            &texture,
            right_panel,
            source,
            0.8,
            SourceAlpha::Ignore,
            None,
        ) {
            return fail("failed to draw opaque texture", error);
        }
        if let Err(error) = queue.submit_composition(&composition) {
            return fail("composition submit failed", error);
        }
        if let Err(error) = image.present(&display) {
            return fail("image present failed", error);
        }

        frame = frame.wrapping_add(1);
        thread::sleep(FRAME_INTERVAL);
    }
}
