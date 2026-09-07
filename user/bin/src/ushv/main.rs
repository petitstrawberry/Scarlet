//! U-SHV - Userspace Virtual Machine Monitor for Scarlet
//!
//! U-SHV is the userspace component of Scarlet's Type-2 hypervisor architecture.
//! It handles device emulation, guest management, and I/O processing while the
//! kernel (SHV) handles privileged VM operations.
//!
//! # Usage
//!
//! ```bash
//! ushv [options] <guest_image>
//!
//! Options:
//!   -h, --help          Show this help message
//!   -m, --memory <size> Guest memory size in MB (default: 256)
//!   -i, --initramfs     Path to initramfs/initrd for guest
//! ```
//!
//! # Supported Guest Types
//!
//! - Raw binary images (e.g., simple bare-metal programs)
//! - ELF executables with SBI firmware support
//!
//! # Architecture
//!
//! Supported on RISC-V 64-bit and AArch64.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;

#[cfg(target_arch = "riscv64")]
mod riscv64;

#[cfg(target_arch = "aarch64")]
mod aarch64;

mod device;
mod devices;
mod machine;

#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
fn main() -> i32 {
    riscv64::run()
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
fn main() -> i32 {
    aarch64::run()
}
