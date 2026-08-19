//! Compatibility facade for existing Scarlet SGFX applications.
//!
//! New cross-platform consumers should depend on `sgfx-core` plus an explicit
//! backend. This facade preserves the former `sgfx` package while Scarlet
//! applications migrate to explicit platform composition.

#![cfg_attr(not(feature = "std"), no_std)]

pub use scarlet_sgfx_virgl::*;
