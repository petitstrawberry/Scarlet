//! VFS v2 Filesystem Drivers
//!
//! This module contains all filesystem drivers that implement the VFS v2
//! FileSystemOperations interface. Each driver provides a specific filesystem
//! type with its own characteristics and use cases.
//!
//! ## Available Drivers
//!
//! - **tmpfs**: Memory-based temporary filesystem with optional size limits
//! - **cpiofs**: Read-only CPIO archive filesystem for initramfs
//! - **overlayfs**: Union/overlay filesystem combining multiple layers
//! - **initramfs**: Helper module for mounting initramfs during boot
//! - **devfs**: Device filesystem that automatically exposes all registered devices
//! - **fat32**: FAT32 filesystem driver for block devices
//! - **ext2**: Ext2 filesystem driver for block devices
//!
//! ## Adding New Drivers
//!
//! To add a new filesystem driver:
//!
//! 1. Create a new module implementing `FileSystemOperations`
//! 2. Implement `VfsNode` for your filesystem's node type
//! 3. Add a driver struct implementing `FileSystemDriverV2`
//! 4. Register the driver in the driver manager during initialization
//!
//! ## Driver Registration
//!
//! All drivers in this module automatically register themselves using
//! the `driver_initcall!` macro during kernel initialization.

pub mod overlayfs;
pub mod cpiofs;
pub mod tmpfs;
pub mod initramfs;
pub mod devfs;
pub mod fat32;
pub mod ext2;