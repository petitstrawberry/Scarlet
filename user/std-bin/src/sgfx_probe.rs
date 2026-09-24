//! GPU capability probe utility.

use sgfx::Instance;
use std::process::ExitCode;

fn main() -> ExitCode {
    let instance = match Instance::new() {
        Ok(instance) => instance,
        Err(error) => {
            println!("failed to select an SGFX backend: {}", error);
            return ExitCode::FAILURE;
        }
    };
    let device = match instance.open_device("/dev/gpu0") {
        Ok(device) => device,
        Err(error) => {
            println!("failed to open /dev/gpu0: {:?}", error);
            return ExitCode::FAILURE;
        }
    };
    println!("SGFX backend: {}", device.backend());

    let capabilities = device.capabilities();
    println!("GPU capabilities:");
    println!("  rendering: {}", capabilities.supports_rendering());
    println!("  presentation: {}", capabilities.supports_presentation());

    if let Err(error) = device.create_context() {
        println!("failed to create GPU context: {:?}", error);
        return ExitCode::FAILURE;
    }
    println!("  graphics context: available");

    ExitCode::SUCCESS
}
