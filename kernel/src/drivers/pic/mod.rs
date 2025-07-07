//! Platform Interrupt Controller (PIC) implementations
//!
//! This module contains implementations of various interrupt controllers
//! used in different platforms and architectures.

pub mod plic;
pub mod clint;

pub use plic::Plic;
pub use clint::Clint;
