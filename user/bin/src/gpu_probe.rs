//! GPU probe utility.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use gpu::{GPU_BUFFER_FLAG_CPU_VISIBLE, GPU_RESULT_SUCCESS, Gpu};
use std::println;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let gpu = match Gpu::open("/dev/gpu0") {
        Ok(gpu) => gpu,
        Err(e) => {
            println!("failed to open /dev/gpu0: {:?}", e);
            return 1;
        }
    };

    let info = match gpu.query_info() {
        Ok(info) => info,
        Err(e) => {
            println!("failed to query GPU information: {:?}", e);
            return 1;
        }
    };

    println!("GPU information:");
    println!("  result: {}", info.result);
    if info.result != GPU_RESULT_SUCCESS {
        return 1;
    }
    println!("  device_state: {}", info.device_state);
    println!("  execution_support: 0x{:x}", info.execution_support);
    println!(
        "  max_opaque_command_size: {}",
        info.max_opaque_command_size
    );
    println!("  backend_feature_bits: 0x{:x}", info.backend_feature_bits);
    match core::str::from_utf8(info.backend_id_bytes()) {
        Ok(identifier) => println!("  backend_id: {}", identifier),
        Err(_) => println!("  backend_id: {:?}", info.backend_id_bytes()),
    }
    println!("  backend_info: {:?}", info.backend_info_bytes());

    let buffer = match gpu.create_buffer(4096, GPU_BUFFER_FLAG_CPU_VISIBLE) {
        Ok(buffer) => buffer,
        Err(e) => {
            println!("failed to create GPU buffer: {:?}", e);
            return 1;
        }
    };
    let buffer_info = match buffer.query_info() {
        Ok(info) => info,
        Err(e) => {
            println!("failed to query GPU buffer: {:?}", e);
            return 1;
        }
    };
    println!(
        "GPU buffer: size={} cpu_visible={}",
        buffer_info.size_bytes, buffer_info.cpu_visible
    );

    let timeline = match gpu.create_timeline(1) {
        Ok(timeline) => timeline,
        Err(e) => {
            println!("failed to create GPU timeline: {:?}", e);
            return 1;
        }
    };
    let point = match timeline.create_point(2) {
        Ok(point) => point,
        Err(e) => {
            println!("failed to create GPU timeline point: {:?}", e);
            return 1;
        }
    };
    let signal = match timeline.signal(2) {
        Ok(signal) => signal,
        Err(e) => {
            println!("failed to signal GPU timeline: {:?}", e);
            return 1;
        }
    };
    println!(
        "GPU timeline: value={} point_target={}",
        signal.current_value,
        point.target_value()
    );

    0
}
