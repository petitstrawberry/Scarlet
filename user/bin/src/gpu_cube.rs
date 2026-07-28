//! Standalone accelerated GPU display sample.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::vec::Vec;
use core::time::Duration;

use framebuffer::DisplaySurface;
use gpu::{GPU_EXECUTION_SUPPORT_PRESENTATION, GPU_EXECUTION_SUPPORT_QUEUE, Gpu};
use std::println;

const VIRGL_CCMD_CREATE_OBJECT: u32 = 1;
const VIRGL_CCMD_SET_FRAMEBUFFER_STATE: u32 = 5;
const VIRGL_CCMD_CLEAR: u32 = 7;
const VIRGL_OBJECT_SURFACE: u32 = 8;
const VIRGL_FORMAT_B8G8R8A8_UNORM: u32 = 1;
const PIPE_CLEAR_COLOR0: u32 = 1 << 2;
const SURFACE_HANDLE: u32 = 2;

fn command_header(command: u32, object: u32, payload_dwords: u32) -> u32 {
    command | (object << 8) | (payload_dwords << 16)
}

fn push_dword(commands: &mut Vec<u8>, value: u32) {
    commands.extend_from_slice(&value.to_le_bytes());
}

fn build_clear_commands(resource_id: u32, color: [f32; 4], initialize: bool) -> Vec<u8> {
    let mut commands = Vec::with_capacity((6 + 4 + 9) * core::mem::size_of::<u32>());

    if initialize {
        push_dword(
            &mut commands,
            command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SURFACE, 5),
        );
        push_dword(&mut commands, SURFACE_HANDLE);
        push_dword(&mut commands, resource_id);
        push_dword(&mut commands, VIRGL_FORMAT_B8G8R8A8_UNORM);
        push_dword(&mut commands, 0);
        push_dword(&mut commands, 0);

        push_dword(
            &mut commands,
            command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3),
        );
        push_dword(&mut commands, 1);
        push_dword(&mut commands, 0);
        push_dword(&mut commands, SURFACE_HANDLE);
    }

    push_dword(&mut commands, command_header(VIRGL_CCMD_CLEAR, 0, 8));
    push_dword(&mut commands, PIPE_CLEAR_COLOR0);
    for component in color {
        push_dword(&mut commands, component.to_bits());
    }
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);

    commands
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let display = match DisplaySurface::open_primary() {
        Ok(display) => display,
        Err(error) => {
            println!("gpu_cube: failed to open primary display: {:?}", error);
            return 1;
        }
    };
    let display_info = match display.get_info() {
        Ok(info) => info,
        Err(error) => {
            println!("gpu_cube: failed to query display: {:?}", error);
            return 1;
        }
    };
    let gpu = match Gpu::open("/dev/gpu0") {
        Ok(gpu) => gpu,
        Err(error) => {
            println!("gpu_cube: failed to open GPU: {:?}", error);
            return 1;
        }
    };
    let gpu_info = match gpu.query_info() {
        Ok(info) => info,
        Err(error) => {
            println!("gpu_cube: failed to query GPU: {:?}", error);
            return 1;
        }
    };
    let required_support = GPU_EXECUTION_SUPPORT_QUEUE | GPU_EXECUTION_SUPPORT_PRESENTATION;
    if gpu_info.execution_support & required_support != required_support {
        println!(
            "gpu_cube: queue/presentation unsupported: 0x{:x}",
            gpu_info.execution_support
        );
        return 1;
    }

    let dialect = match gpu.query_dialect(0) {
        Ok(dialect) => dialect,
        Err(error) => {
            println!("gpu_cube: failed to query dialect: {:?}", error);
            return 1;
        }
    };
    let context = match gpu.create_context(&dialect) {
        Ok(context) => context,
        Err(error) => {
            println!("gpu_cube: failed to create context: {:?}", error);
            return 1;
        }
    };
    let image = match gpu.create_image(display_info.width, display_info.height) {
        Ok(image) => image,
        Err(error) => {
            println!("gpu_cube: failed to create image: {:?}", error);
            return 1;
        }
    };
    let resource_token = match context.attach_image(&image) {
        Ok(token) => token,
        Err(error) => {
            println!("gpu_cube: failed to attach image: {:?}", error);
            return 1;
        }
    };
    let resource_id = match u32::try_from(resource_token) {
        Ok(resource_id) if resource_id != 0 => resource_id,
        _ => {
            println!("gpu_cube: backend resource token is not a VirGL resource ID");
            return 1;
        }
    };
    let queue = match context.create_queue() {
        Ok(queue) => queue,
        Err(error) => {
            println!("gpu_cube: failed to create queue: {:?}", error);
            return 1;
        }
    };

    println!(
        "gpu_cube: presenting {}x{} VirGL render target (resource {})",
        display_info.width, display_info.height, resource_id
    );

    let colors = [
        [0.85, 0.08, 0.08, 1.0],
        [0.08, 0.70, 0.18, 1.0],
        [0.08, 0.20, 0.85, 1.0],
    ];
    let mut frame = 0usize;
    loop {
        let commands = build_clear_commands(resource_id, colors[frame % colors.len()], frame == 0);
        if let Err(error) = queue.submit(&commands) {
            println!("gpu_cube: VirGL clear failed: {:?}", error);
            return 1;
        }
        if let Err(error) = display.present_image(image.as_handle(), None) {
            println!("gpu_cube: image present failed: {:?}", error);
            return 1;
        }
        frame = frame.wrapping_add(1);
        std::thread::sleep(Duration::from_millis(750));
    }
}
