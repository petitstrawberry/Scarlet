//! Capability traits and implementations for Scarlet Native API
//!
//! This module defines the various capabilities that KernelObjects can support.
//! Capabilities provide type-safe, operation-specific interfaces to KernelObjects.
//!
//! ## Available Capabilities
//!
//! - **StreamOps**: Read/write operations for streaming data (borrowed from `Handle`)
//! - **FileObject**: File-specific operations (seek, truncate, metadata) (borrowed)
//! - **SocketObject**: Socket-specific operations (bind, listen, connect, accept) (borrowed)
//! - **SharedMemoryObject**: Shared-memory object marker and operations (borrowed)
//! - **MemoryMappingOps**: Memory mapping operations (mmap, munmap) (borrowed)
//!
//! ## Design Philosophy
//!
//! - Each capability focuses on a specific set of related operations
//! - Capabilities are composable - one KernelObject may support multiple capabilities
//! - Type safety prevents calling unsupported operations
//! - Direct syscall mapping for zero-cost abstractions

pub mod file;
pub mod memory_mapping;
pub mod shared_memory;
pub mod socket;
pub mod stream;

// Re-export capability types for convenience
pub use file::{FileError, FileMetadata, FileObject, FileResult, SeekFrom};
pub use memory_mapping::MemoryMappingOps;
pub use shared_memory::{SharedMemoryObject, SharedMemoryObjectError, SharedMemoryObjectResult};
pub use socket::{ShutdownHow, SocketObject, SocketObjectError, SocketObjectResult};
pub use stream::{StreamError, StreamOps, StreamResult};
