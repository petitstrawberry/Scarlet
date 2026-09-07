#[cfg(target_arch = "riscv64")]
pub mod plic;
pub mod uart;

#[cfg(target_arch = "aarch64")]
pub mod gic;
