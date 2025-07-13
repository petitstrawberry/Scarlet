//! Synchronization primitives module
//!
//! This module provides various synchronization primitives for the Scarlet kernel,
//! including the Waker mechanism for asynchronous task waiting and waking.

pub mod waker;

pub use waker::Waker;
