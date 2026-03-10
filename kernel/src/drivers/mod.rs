//! Device drivers module.
//!
//! This module encapsulates various device drivers for the Scarlet kernel.

pub mod block;
pub mod graphics;
pub mod network;
pub mod pic;
pub mod special;
pub mod uart;
pub mod usb;
pub mod virtio;
pub mod virtio_input;
pub mod virtio_rng;
