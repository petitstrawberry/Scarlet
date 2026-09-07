//! Print the running kernel's name, version, and build target via its native syscall.

use std::process::ExitCode;

use scarlet_os::system;

fn main() -> ExitCode {
    let info = match system::kernel_info() {
        Ok(info) => info,
        Err(error) => {
            eprintln!("kernel-info: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("{} {} ({})", info.name, info.version, info.target);
    ExitCode::SUCCESS
}
