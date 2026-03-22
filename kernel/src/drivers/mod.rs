//! Device drivers module.
//!
//! This module encapsulates various device drivers for the Scarlet kernel.

pub mod block;
pub mod graphics;
#[cfg(target_arch = "aarch64")]
pub mod i2c;
#[cfg(target_arch = "aarch64")]
pub mod iommu;
pub mod network;
#[cfg(target_arch = "aarch64")]
pub mod nvmem;
#[cfg(target_arch = "aarch64")]
pub mod pcie;
#[cfg(target_arch = "aarch64")]
pub mod phy;
pub mod pic;
#[cfg(target_arch = "aarch64")]
pub mod soc;
pub mod special;
pub mod uart;
pub mod usb;
pub mod virtio;
pub mod virtio_input;
pub mod virtio_rng;
pub mod watchdog;
