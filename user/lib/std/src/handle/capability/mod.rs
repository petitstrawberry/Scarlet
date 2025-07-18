//! Capability traits and implementations for Scarlet Native API
//!
//! This module defines the various capabilities that KernelObjects can support.
//! Capabilities provide type-safe, operation-specific interfaces to KernelObjects.
//!
//! ## Available Capabilities
//!
//! - **StreamOps**: Read/write operations for streaming data
//! - **FileObject**: File-specific operations (seek, truncate, metadata)
//!
//! ## Design Philosophy
//!
//! - Each capability focuses on a specific set of related operations
//! - Capabilities are composable - one KernelObject may support multiple capabilities
//! - Type safety prevents calling unsupported operations
//! - Direct syscall mapping for zero-cost abstractions

pub mod stream;
pub mod file;

// Re-export capability types for convenience
pub use stream::{StreamOps, StreamError, StreamResult};
pub use file::{FileObject, FileError, FileResult, SeekFrom, FileMetadata};
