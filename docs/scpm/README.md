# SCPM (Scarlet Package Manager)

SCPM is a minimal package manager for the Scarlet operating system.

## Architecture

### Components

- **Library** (`user/lib/scpm/`) - Core package management functionality
- **CLI** (`user/bin/src/scpm.rs`) - Command-line interface
- **Builder** (`tools/scpm-pack/`) - Host-side package builder tool

### Package Format

SCPM packages use `.scarlet` archive format (tar.gz) containing:

```
package.toml          # Package metadata
bin/                  # Executable binaries
lib/                  # Shared libraries (optional)
```

### Package Metadata (package.toml)

```toml
name = "hello"
version = "0.1.0"
description = "Hello World example package"
author = "Scarlet Team"
architecture = "riscv64"  # or "aarch64", or "any"
binaries = ["hello"]
dependencies = []  # List of required packages
```

### Storage Layout

```
/var/scpm/
  ├── installed/          # Registry of installed packages
  ├── cache/             # Downloaded package archives
  └── repository/        # Package repository index
```

## Usage

### Scarlet OS (User Space)

```bash
# List installed packages
scpm list

# Show package information
scpm info hello

# Install a package
scpm install hello-0.1.0.scarlet

# Remove a package
scpm remove hello

# Search for packages
scpm search editor

# Update repository
scpm update
```

### Host Machine (Package Building)

```bash
# Initialize a new package
cd tools/scpm-pack
cargo build --release

./target/release/scpm-pack init hello --dir ./hello

# Build package
./target/release/scpm-pack build ./hello
# Creates: hello-0.1.0.scarlet
```

## Library API

```rust
use scpm::{Config, PackageManager, Package, PackageMetadata};

// Create package manager with default configuration
let manager = PackageManager::with_default_config();

// Check if package is installed
if manager.is_installed("hello") {
    println!("Package is installed");
}

// Get package information
if let Some(pkg) = manager.get_installed("hello") {
    println!("Version: {}", pkg.version);
}

// List all installed packages
for pkg in manager.list_installed() {
    println!("{} - {}", pkg.name, pkg.version);
}
```

## Implementation Status

### Completed
- [x] Core library types (Config, Error, Package, PackageMetadata)
- [x] Package manager operations (install, remove, list, info)
- [x] Repository support (search, fetch)
- [x] Archive operations (PackageArchive stub)
- [x] Host-side package builder (init, build)
- [x] Build configuration integration

### Pending
- [ ] Archive extraction and installation
- [ ] Binary and library deployment
- [ ] Repository file I/O operations
- [ ] Network repository support (HTTP fetch)
- [ ] CLI binary re-enable (serde no_std compatibility fix)

## Development Notes

### No_std Considerations

The SCPM library uses `no_std` and runs in the Scarlet user-space environment:

- Uses `scarlet_std` instead of `std`
- Uses `alloc` for dynamic allocations
- No serde serialization in CLI (uses custom parsing)
- Simple string-based configuration parsing

### Serde No_std Issue

The CLI binary is currently disabled in `user/bin/Cargo.toml` due to serde's `no_std` compatibility issues. The library builds successfully but the CLI cannot use serde for parsing.

**Solutions to consider:**
1. Use a custom TOML parser (simple string parsing like `scarlet-desktop-config`)
2. Implement proper package metadata parsing
3. Re-enable CLI once serialization is working

### Reference Examples

See existing Scarlet binaries for patterns:
- `user/bin/src/settings.rs` - Simple TOML parsing without serde
- `user/bin/src/netcfgd/main.rs` - Custom config parser
- `user/bin/src/stemd/main.rs` - Service management patterns
