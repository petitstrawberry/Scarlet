//! External display drivers for Apple Silicon (DCPext, DPXBAR).
//!
//! Disabled until the kernel architecture supports the required prerequisites:
//! - Late/delayed driver probing (after interrupt subsystem and scheduler init)
//! - Driver dependency graph (DART → PMGR → CLK → DCPext ordering)
//! - Interrupt-driven EPIC message handling (AFK ring buffer, service announces)
//! - DART IOMMU setup that coexists with m1n1's pre-initialized page tables
//!
//! The driver code itself is complete (I2C → EFUSE → ASC → RTKit → AFK → EPIC
//! → DCPext → DPXBAR → ATC PHY DP → DisplayOutput). What's missing is the kernel
//! infrastructure to initialize coprocessors safely after the boot interrupt
//! and scheduler are ready.
//!
//! TODO: Implement late_probe mechanism so coprocessor drivers can init after
//!       interrupt/scheduler are available.
//! TODO: Resolve DART page table ownership — m1n1 locks dart-disp0 and pre-sets
//!       page tables before handing off to the OS. Scarlet must either inherit
//!       or rebuild these tables.
//! TODO: Add deferred probe support (equivalent to Linux's deferred_probe)
//!       so DART can probe before DCPext without ordering hacks.

// #[cfg(target_arch = "aarch64")]
// pub mod dcpext;
//
// #[cfg(target_arch = "aarch64")]
// pub mod dpxbar;
