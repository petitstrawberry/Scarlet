//! GPU capability probe utility.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use gpu::Device;
use std::println;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let device = match Device::open("/dev/gpu0") {
        Ok(device) => device,
        Err(error) => {
            println!("failed to open /dev/gpu0: {:?}", error);
            return 1;
        }
    };

    let capabilities = device.capabilities();
    println!("GPU capabilities:");
    println!("  rendering: {}", capabilities.supports_rendering());
    println!("  presentation: {}", capabilities.supports_presentation());

    let context = match device.create_context() {
        Ok(context) => context,
        Err(error) => {
            println!("failed to create GPU context: {:?}", error);
            return 1;
        }
    };
    match context.create_queue() {
        Ok(_) => println!("  graphics queue: available"),
        Err(error) => {
            println!("failed to create graphics queue: {:?}", error);
            return 1;
        }
    }

    0
}
