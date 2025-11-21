//! # Linux DRM (Direct Rendering Manager) Compatibility Layer
//!
//! This module provides a Linux DRM-compatible ABI for graphics operations.
//! It maps DRM concepts and ioctls to Scarlet's OS-independent GraphicsDevice
//! trait and related abstractions.
//!
//! ## Design Philosophy
//!
//! The DRM layer is strictly isolated within the `kernel/src/abi/linux` directory
//! and serves as an adapter between Linux's DRM interface and Scarlet's core
//! graphics abstractions. This design allows:
//!
//! - Linux applications using DRM to run on Scarlet
//! - Core graphics code to remain OS-independent
//! - Other OS ABIs (Windows, macOS, etc.) to be added without affecting core code
//!
//! ## Architecture
//!
//! ```text
//! Linux Application
//!       |
//!       | DRM ioctls
//!       v
//! +------------------+
//! | DRM Compat Layer | <-- This module
//! +------------------+
//!       |
//!       | Trait calls
//!       v
//! +------------------+
//! | GraphicsDevice   | <-- OS-independent
//! | PageFlipCapable  |
//! +------------------+
//!       |
//!       v
//! Device Drivers (VirtIO GPU, etc.)
//! ```
//!
//! ## MVP Implementation
//!
//! The initial MVP focuses on basic DRM operations:
//!
//! - **DRM_IOCTL_VERSION**: Report DRM driver version
//! - **DRM_IOCTL_MODE_GETRESOURCES**: List available connectors and CRTCs
//! - **DRM_IOCTL_MODE_GETCRTC/SETCRTC**: Get/set CRTC configuration
//! - **DRM_IOCTL_MODE_CREATE_DUMB**: Create dumb buffer
//! - **DRM_IOCTL_MODE_MAP_DUMB**: Map dumb buffer to user space
//! - **DRM_IOCTL_MODE_DESTROY_DUMB**: Destroy dumb buffer
//! - **DRM_IOCTL_MODE_PAGE_FLIP**: Simple page flip (initially copy+flush)
//!
//! ## Future Extensions
//!
//! Future work may include:
//! - Hardware-accelerated page flipping
//! - 3D rendering capabilities
//! - Multiple display support
//! - Advanced synchronization (vblank events, etc.)

pub mod types;
pub mod ioctls;
pub mod file;

pub use types::*;
pub use ioctls::*;
pub use file::DrmFile;
