//! GPU probe utility.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use gpu::Gpu;
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

    let capabilities = match gpu.capabilities() {
        Ok(capabilities) => capabilities,
        Err(e) => {
            println!("failed to query GPU capabilities: {:?}", e);
            return 1;
        }
    };

    println!("GPU capabilities:");
    println!("  feature_bits: 0x{:x}", capabilities.feature_bits);
    println!("  capsets: {}", capabilities.capset_count);
    println!("  virgl: {}", capabilities.supports_virgl());

    for index in 0..capabilities.capset_count {
        match gpu.capset_info(index) {
            Ok(info) => {
                println!(
                    "  capset[{}]: id={} max_version={} max_size={}",
                    index, info.id, info.max_version, info.max_size
                );
                if info.max_size != 0 {
                    match gpu.read_capset(info.id, info.max_version, info.max_size as usize) {
                        Ok(bytes) => println!("    read {} bytes", bytes.len()),
                        Err(e) => println!("    read failed: {:?}", e),
                    }
                }
            }
            Err(e) => println!("  capset[{}] failed: {:?}", index, e),
        }
    }

    if capabilities.supports_virgl() {
        let context_id = 1;
        match gpu.create_context(context_id, "gpu_probe") {
            Ok(()) => {
                println!("created context {}", context_id);
                let _ = gpu.destroy_context(context_id);
            }
            Err(e) => println!("context create failed: {:?}", e),
        }
    }

    0
}
