//! Synchronization primitives module
//!
//! This module provides various synchronization primitives for the Scarlet kernel.
//! External modules should use these re-exports instead of depending on `spin` directly,
//! so that the kernel can control the underlying lock implementation.

pub mod waker;

pub use waker::Waker;

pub use spin::{Mutex, MutexGuard, Once, RwLock, RwLockReadGuard, RwLockWriteGuard};
