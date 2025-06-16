//! Transparent Executor Module
//!
//! This module implements the TransparentExecutor, which provides unified
//! exec API for all ABIs in Scarlet OS.
//! 
//! The TransparentExecutor enables:
//! - Unified exec processing for all ABIs
//! - Binary format detection and ABI delegation
//! - VFS inheritance and resource management
//! - Does NOT contain ABI-specific knowledge
//!
//! ## Design Principle
//!
//! The TransparentExecutor follows the principle of separation of concerns:
//! - **Scarlet Core**: Provides unified exec API and resource management
//! - **ABI Modules**: Handle their own binary formats and conversions
//! - **No ABI knowledge in core**: Core does not know about specific ABIs

pub mod executor;

pub use executor::TransparentExecutor;
