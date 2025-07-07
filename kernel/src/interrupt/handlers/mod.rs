//! Standard interrupt handlers for common devices and system functions.
//!
//! This module contains implementations of interrupt handlers for various
//! system components like timers, UARTs, and other devices.

pub mod timer;

pub use timer::TimerInterruptHandler;
