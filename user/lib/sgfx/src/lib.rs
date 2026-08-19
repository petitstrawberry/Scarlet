//! Cross-platform SGFX graphics contracts and intermediate representation.
//!
//! This crate owns only backend-neutral resource descriptions and command
//! buffers. Backend implementations and operating-system transports live in
//! separate crates so consumers can select them at their composition root.

#![no_std]

extern crate alloc;

/// Backend-neutral logical graphics intermediate representation.
pub mod ir;
