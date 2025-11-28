//! Platform Interrupt Controller (PIC) implementations
//!
//! This module contains implementations of various interrupt controllers
//! used in different platforms and architectures.

pub mod clint;
pub mod plic;

pub use clint::Clint;
pub use plic::Plic;
