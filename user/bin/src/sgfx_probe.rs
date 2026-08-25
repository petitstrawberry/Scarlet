//! GPU capability probe utility.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use sgfx::Instance;
use std::println;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let instance = match Instance::new() {
        Ok(instance) => instance,
        Err(error) => {
            println!("failed to select an SGFX backend: {}", error);
            return 1;
        }
    };
    let device = match instance.open_device("/dev/gpu0") {
        Ok(device) => device,
        Err(error) => {
            println!("failed to open /dev/gpu0: {:?}", error);
            return 1;
        }
    };
    println!("SGFX backend: {}", device.backend());

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
    let _ = context;
    println!("  graphics context: available");

    0
}
