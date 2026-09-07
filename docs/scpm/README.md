# SCPM (Scarlet Package Manager)

SCPM is a simple package manager for Scarlet OS.

## Architecture

### Components

- **Library** (`user/lib/scpm/`) - Core functionality (package management, archive extraction)
- **CLI** (`user/bin/src/scpm.rs`) - Command-line interface
- **Builder** (`tools/scpm-pack/`) - Host-side package creation tool

## Package Format

SCPM packages use `.scarlet` format (tar.gz):

```
package-0.1.0.scarlet (tar.gz)
├── package.toml          # Metadata (auto-parsed)
├── bin/                  # Executables
│   └── hello
├── lib/                  # Shared libraries
└── ...                   # Other files
```

### package.toml

```toml
[package]
name = "hello"
version = "0.1.0"
description = "Hello World example package"
author = "Scarlet Team"
architecture = "riscv64"  # "riscv64", "aarch64", "any"
binaries = ["hello"]
libraries = []
dependencies = []
```

### File Placement

During installation, files under the `scarlet/` directory are extracted to the system root:

```
scarlet/                  # Prefix in package
├── bin/hello            → /bin/hello
├── lib/libtest.so       → /lib/libtest.so
└── etc/config.conf      → /etc/config.conf
```

## Usage

### Scarlet OS (User Space)

```bash
# List installed packages
scpm list

# Show package information
scpm info hello

# Install package
scpm install hello-0.1.0.scarlet

# Remove package
scpm remove hello

# Search packages
scpm search editor
```

### Host Machine (Package Creation)

```bash
# Build
cd tools/scpm-pack
cargo build --release

# Initialize new package
./target/release/scpm-pack init hello \
  --dir ./hello \
  --description "Test package" \
  --author "Developer"

# Place binaries
cp /path/to/hello ./hello/bin/

# Build package
./target/release/scpm-pack build ./hello --output hello-0.1.0.scarlet
```

## Library API

```rust
use scpm::{PackageManager, PackageMetadata};

// Create package manager with default config
let manager = PackageManager::with_default_config();

// Check if installed
if manager.is_installed("hello") {
    println!("Installed!");
}

// Get package info
if let Some(pkg) = manager.get_installed("hello") {
    println!("Version: {}", pkg.version);
}

// Install from bytes
let data = std::fs::read("package.scarlet")?;
manager.install_from_bytes("package", &data)?;
```

## Implementation Status

### Completed ✅
- [x] Core library types (PackageMetadata, PackageArchive)
- [x] Package manager operations (install, remove, list, info)
- [x] tar.gz archive extraction (miniz_oxide + tar-no-std)
- [x] File extraction and placement (scarlet/ → system root)
- [x] Installed file tracking
- [x] Overwrite prevention
- [x] Host-side package builder
- [x] CLI binary

### Not Implemented 📋
- [ ] Remote repository support (HTTP fetch)
- [ ] Dependency resolution
- [ ] Configuration file management (conffiles)
- [ ] Upgrade handling
- [ ] Database persistence (currently in-memory only)

## Technical Details

### Dependencies

```toml
[dependencies]
scarlet_std = { path = "../std" }
miniz_oxide = { version = "0.7", features = ["with-alloc"] }
tar-no-std = "0.4"
```

### No_std Environment

- Uses `scarlet_std` (replaces standard library)
- Dynamic memory allocation with `alloc`
- Custom TOML parser (no serde)

### Archive Processing Flow

1. **gzip decompression** - deflate expansion with miniz_oxide
2. **tar parsing** - entry traversal with tar-no-std
3. **Metadata extraction** - parse package.toml
4. **File placement** - extract scarlet/ contents to system root

## Design Principles

### Simplicity
- Minimal dependencies
- Clear package structure
- Maintainable code

### Safety
- Prevent overwriting existing files
- Track installed files
- Atomic operations (future)

### Extensibility
- Plugin format (.deb compatibility considered)
- Declarative hook scripts
- Remote repository support
