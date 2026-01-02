pub mod device;
pub mod generic;
#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64::LinuxRiscv64Abi as LinuxAbi;
#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::LinuxAarch64Abi as LinuxAbi;
