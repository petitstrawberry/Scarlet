//! GPU probe utility.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use gpu::{GPU_BUFFER_FLAG_CPU_VISIBLE, GPU_EXECUTION_SUPPORT_QUEUE, GPU_RESULT_SUCCESS, Gpu};
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

    if info.execution_support & GPU_EXECUTION_SUPPORT_QUEUE == 0 {
        println!("GPU execution queues are not supported");
        return 1;
    }

    let dialect = match gpu.query_dialect(0) {
        Ok(dialect) => dialect,
        Err(e) => {
            println!("failed to query GPU execution dialect: {:?}", e);
            return 1;
        }
    };
    println!(
        "GPU dialect: index={} token=0x{:x} info_bytes={}",
        dialect.index(),
        dialect.token(),
        dialect.opaque_info().len()
    );

    let context = match gpu.create_context(&dialect) {
        Ok(context) => context,
        Err(e) => {
            println!("failed to create GPU execution context: {:?}", e);
            return 1;
        }
    };
    let queue = match context.create_queue() {
        Ok(queue) => queue,
        Err(e) => {
            println!("failed to create GPU execution queue: {:?}", e);
            return 1;
        }
    };
    println!(
        "GPU queue: max_opaque_command_size={}",
        queue.max_opaque_command_size()
    );

    let timeline = match gpu.create_timeline(0) {
        Ok(timeline) => timeline,
        Err(e) => {
            println!("failed to create GPU timeline: {:?}", e);
            return 1;
        }
    };
    let point = match timeline.create_point(1) {
        Ok(point) => point,
        Err(e) => {
            println!("failed to create GPU timeline point: {:?}", e);
            return 1;
        }
    };

    const VIRGL_CCMD_NOP: [u8; 4] = 0u32.to_le_bytes();
    let submission = match queue.submit_and_signal(&VIRGL_CCMD_NOP, &timeline, 1) {
        Ok(submission) => submission,
        Err(e) => {
            println!("failed to submit VirGL NOP: {:?}", e);
            return 1;
        }
    };
    println!(
        "VirGL NOP completed: timeline_value={} failed={} point_target={}",
        submission.completed_value,
        submission.timeline_failed,
        point.target_value()
    );

    0
}
