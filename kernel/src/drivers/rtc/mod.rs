//! RTC (Real Time Clock) platform drivers.
//!
//! These drivers probe RTC hardware from the device tree and seed the kernel
//! wall-clock epoch via `time::initialize_wall_clock_from_rtc_sample`.

#[cfg(target_arch = "riscv64")]
pub mod goldfish;
#[cfg(target_arch = "aarch64")]
pub mod pl031;
