//! Device drivers module.
//!
//! This module encapsulates various device drivers for the Scarlet kernel.

pub mod block;
pub mod display;
pub mod graphics;
pub mod i2c;
pub mod iommu;
pub mod network;
pub mod nvmem;
pub mod pcie;
pub mod phy;
pub mod pic;
pub mod soc;
pub mod special;
pub mod spi;
pub mod uart;
pub mod usb;
pub mod virtio;
pub mod virtio_input;
pub mod virtio_rng;
pub mod watchdog;
pub mod gpio;
