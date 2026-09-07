//! MMC, SD, and eMMC drivers.
//!
//! The protocol layer is independent of the host controller. SDHCI provides
//! the first controller implementation and is bound to QEMU through PCI.

pub mod core;
pub mod sdhci;
