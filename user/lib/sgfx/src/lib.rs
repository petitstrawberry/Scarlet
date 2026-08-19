//! Cross-platform SGFX graphics contracts and intermediate representation.
//!
//! This crate owns only backend-neutral resource descriptions and command
//! buffers. Backend implementations and operating-system transports live in
//! separate crates so consumers can select them at their composition root.
//! Backends own resource materialization, validation of their supported IR
//! subset, command lowering, transport budgeting, and submission. Portable
//! renderers therefore record complete logical passes without encoding limits
//! from any particular device transport.

#![no_std]

extern crate alloc;

/// Contracts implemented by SGFX command execution backends.
pub mod backend;

/// Backend-neutral logical graphics intermediate representation.
pub mod ir;
