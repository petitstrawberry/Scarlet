# ABI Zone Implementation

This document describes the implementation of the ABI Zone feature in the Scarlet kernel.

## Overview

The ABI Zone feature allows dynamically switching the ABI (Application Binary Interface) used for system call handling based on the program counter (PC) address when the system call is issued. This enables different parts of a program to use different ABIs, facilitating better compatibility and interoperability between binaries compiled for different ABIs.

## Architecture

### Core Components

1. **AbiZone struct** (`kernel/src/task/mod.rs`)
   - Represents a memory range with an associated ABI module
   - Contains:
     - `range: Range<usize>` - The memory address range
     - `abi: Box<dyn AbiModule + Send + Sync>` - The ABI module to use for this range

2. **Task struct modifications** (`kernel/src/task/mod.rs`)
   - `default_abi: Box<dyn AbiModule + Send + Sync>` - The default ABI for the task (determined from ELF OSABI)
   - `abi_zones: BTreeMap<usize, AbiZone>` - Map of registered ABI zones, keyed by start address

3. **resolve_abi_mut method** (`kernel/src/task/mod.rs`)
   - Efficiently resolves which ABI to use for a given address
   - Uses BTreeMap's `range_mut()` for O(log n) lookup
   - Falls back to `default_abi` if no zone matches

4. **syscall_dispatcher** (`kernel/src/abi/mod.rs`)
   - Modified to use PC from trapframe to resolve the appropriate ABI
   - Calls `task.resolve_abi_mut(pc)` to get the correct ABI module
   - Delegates system call handling to the resolved ABI

### System Calls

Two new system calls are implemented in the Scarlet Native ABI:

1. **SYS_REGISTER_ABI_ZONE (90)**
   ```rust
   sys_register_abi_zone(start: usize, len: usize, abi_name_ptr: *const u8) -> usize
   ```
   - Registers a new ABI zone for the given memory range
   - `start` - Start address of the memory range
   - `len` - Length of the memory range in bytes
   - `abi_name_ptr` - Pointer to null-terminated ABI name string in user space
   - Returns 0 on success, -1 (usize::MAX) on failure

2. **SYS_UNREGISTER_ABI_ZONE (91)**
   ```rust
   sys_unregister_abi_zone(start: usize) -> usize
   ```
   - Unregisters an ABI zone at the given start address
   - `start` - Start address of the zone to unregister
   - Returns 0 on success, -1 (usize::MAX) on failure

## Implementation Details

### ABI Module Trait Changes

The `AbiModule` trait was updated to include `Send + Sync` bounds:
- `pub trait AbiModule: Send + Sync + 'static`
- `clone_boxed()` now returns `Box<dyn AbiModule + Send + Sync>`
- All implementations (ScarletAbi, Xv6Riscv64Abi) updated accordingly

### Task Cloning

When a task is cloned with `clone_task()`:
- The `default_abi` is cloned using `clone_boxed()`
- All ABI zones are cloned, with each zone's ABI module cloned independently
- Each child task gets its own independent ABI zones

### ABI Resolution Algorithm

```rust
pub fn resolve_abi_mut(&mut self, addr: usize) -> &mut (dyn AbiModule + Send + Sync) {
    // Use BTreeMap range query to find the zone containing addr
    if let Some((_start, zone)) = self.abi_zones.range_mut(..=addr).next_back() {
        if zone.range.contains(&addr) {
            return zone.abi.as_mut();
        }
    }
    // No zone found, return default ABI
    self.default_abi.as_mut()
}
```

This provides efficient O(log n) lookup using BTreeMap's ordered structure.

## Usage Example

```rust
// In user space (pseudocode):

// Register a zone that uses the xv6 ABI for addresses 0x1000-0x2000
let abi_name = "xv6-riscv64\0";
let result = syscall(
    SYS_REGISTER_ABI_ZONE,
    0x1000,        // start address
    0x1000,        // length (4KB)
    abi_name.as_ptr()
);

// Now any system calls issued from code at 0x1000-0x2000 will use the xv6 ABI
// System calls from other addresses will use the default ABI

// Later, unregister the zone
syscall(SYS_UNREGISTER_ABI_ZONE, 0x1000);
```

## Testing

The implementation has been verified to:
- Build successfully without errors
- Not break existing functionality
- Pass all compilation checks

## Future Enhancements

Possible future improvements:
- Add validation to ensure zones don't overlap
- Support for automatic zone detection based on loaded libraries
- Integration with dynamic linker for automatic ABI zone registration
- Statistics and monitoring for ABI zone usage
