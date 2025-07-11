# DevFS Usage Example

This document demonstrates how to use the DevFS (device filesystem) implementation in Scarlet.

## Overview

DevFS is a virtual filesystem that automatically exposes all character and block devices registered in the kernel's DeviceManager. When mounted (typically at `/dev`), it provides device files that can be accessed through standard VFS operations.

## Basic Usage

### 1. Register Devices with DeviceManager

First, devices must be registered with explicit names in the DeviceManager:

```rust
use crate::device::{manager::DeviceManager, char::mockchar::MockCharDevice, block::mockblk::MockBlockDevice};
use alloc::sync::Arc;

// Get the global device manager
let device_manager = DeviceManager::get_manager();

// Register a character device (e.g., TTY)
let tty_device = Arc::new(MockCharDevice::new(1, "tty0"));
device_manager.register_device_with_name("tty0".to_string(), tty_device);

// Register a block device (e.g., disk)  
let disk_device = Arc::new(MockBlockDevice::new(2, "sda", 512, 1000));
device_manager.register_device_with_name("sda".to_string(), disk_device);
```

### 2. Create and Mount DevFS

```rust
use crate::fs::{get_fs_driver_manager, VfsManager};

// Create a VFS manager
let vfs = VfsManager::new();

// Get the filesystem driver manager
let fs_driver_manager = get_fs_driver_manager();

// Create a DevFS instance
let devfs = fs_driver_manager.create_from_option_string("devfs", "").unwrap();

// Mount DevFS at /dev
vfs.mount(devfs, "/dev", 0).unwrap();
```

### 3. Access Device Files

Once mounted, device files can be accessed through standard VFS operations:

```rust
// List devices in /dev
let dev_entries = vfs.readdir("/dev").unwrap();
for entry in dev_entries {
    println!("Device: {} (type: {:?})", entry.name, entry.file_type);
}

// Open a character device
let tty_file = vfs.open("/dev/tty0", O_RDWR).unwrap();

// Open a block device
let disk_file = vfs.open("/dev/sda", O_RDWR).unwrap();
```

## Key Features

### Automatic Device Discovery

DevFS automatically discovers all devices that were registered with names in the DeviceManager:

- **Character Devices**: Appear as `FileType::CharDevice` entries
- **Block Devices**: Appear as `FileType::BlockDevice` entries
- **Other Device Types**: Are filtered out (Generic, Network devices don't appear)

### Dynamic Updates

The device listing is refreshed automatically whenever the filesystem is accessed:

```rust
// Register a new device
let new_tty = Arc::new(MockCharDevice::new(3, "tty1"));
device_manager.register_device_with_name("tty1".to_string(), new_tty);

// DevFS automatically shows the new device on next access
let updated_entries = vfs.readdir("/dev").unwrap();
// Now includes both tty0 and tty1
```

### Read-Only Filesystem

DevFS is read-only, as expected for a device filesystem:

```rust
// These operations will fail with ReadOnly error
let create_result = vfs.create("/dev/newdevice", FileType::RegularFile);
assert!(create_result.is_err());

let remove_result = vfs.remove("/dev/tty0");
assert!(remove_result.is_err());
```

## Device File Information

Each device file contains metadata about the underlying device:

```rust
let metadata = vfs.metadata("/dev/tty0").unwrap();

match metadata.file_type {
    FileType::CharDevice(info) => {
        println!("Character device: ID={}, Type={:?}", info.device_id, info.device_type);
    }
    FileType::BlockDevice(info) => {
        println!("Block device: ID={}, Type={:?}", info.device_id, info.device_type);
    }
    _ => {}
}
```

## Integration with Existing Systems

DevFS integrates seamlessly with the Scarlet kernel's existing device and VFS systems:

- **Device Types**: Uses Scarlet's `DeviceType` enum (Char, Block, Network, Generic)
- **Device IDs**: Uses Scarlet's unique device ID system instead of Unix major/minor numbers
- **VFS v2**: Fully compatible with the modern VFS v2 architecture
- **Mount Operations**: Supports all standard mount options and operations

## Example: Complete Setup

Here's a complete example showing DevFS setup and usage:

```rust
use crate::fs::{get_fs_driver_manager, VfsManager};
use crate::device::{manager::DeviceManager, char::mockchar::MockCharDevice};
use alloc::sync::Arc;

// 1. Register devices
let device_manager = DeviceManager::get_manager();
let console = Arc::new(MockCharDevice::new(1, "console"));
device_manager.register_device_with_name("console".to_string(), console);

// 2. Create VFS and mount DevFS
let vfs = VfsManager::new();
let fs_driver_manager = get_fs_driver_manager();
let devfs = fs_driver_manager.create_from_option_string("devfs", "").unwrap();
vfs.mount(devfs, "/dev", 0).unwrap();

// 3. Use device files
let console_file = vfs.open("/dev/console", O_RDWR).unwrap();
// Now you can read/write to the console device through the VFS
```

This implementation provides a clean, Unix-like device filesystem interface while leveraging Scarlet's modern kernel architecture.