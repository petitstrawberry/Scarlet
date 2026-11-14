#[cfg(target_arch = "aarch64")]
pub mod aarch64;
pub mod device;
pub mod extensions;
pub mod generic;
#[cfg(target_arch = "riscv64")]
pub mod riscv64;
