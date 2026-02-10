# Package Format Specification

## Overview

SCPM packages are distributed as `.scarlet` archive files containing package metadata and payload files.

## Archive Structure

A `.scarlet` file is a tar.gz archive with the following structure:

```
hello-1.0.0.scarlet  (tar.gz)
├── package.toml          # Required: Package metadata
├── bin/                   # Optional: Executable binaries
│   └── hello             # Binary files to install
└── lib/                   # Optional: Shared libraries
    └── libhello.so       # Library files (.so files)
```

## Package Metadata (package.toml)

### Required Fields

| Field | Type | Description | Example |
|-------|------|-------------|---------|
| `name` | String | Package identifier | `"hello"` |
| `version` | String | Semantic versioning | `"1.0.0"` |
| `description` | String | Short description | `"Hello World example"` |
| `architecture` | String | Target architecture | `"riscv64"`, `"aarch64"`, or `"any"` |

### Optional Fields

| Field | Type | Description | Example |
|-------|------|-------------|---------|
| `author` | String | Package maintainer | `"Scarlet Team <team@scarlet.dev>"` |
| `homepage` | String | Project URL | `"https://github.com/scarlet"` |
| `binaries` | Array[String] | List of executable files | `["hello"]` |
| `libraries` | Array[String] | List of library files | `["libhello.so"]` |
| `dependencies` | Array[Dependency] | Required packages | See below |
| `license` | String | License identifier | `"MIT"` |

### Dependencies

Each dependency has:
- `name` (required): Package name
- `version` (optional): Version constraint (not yet enforced)

```toml
[[dependencies]]
name = "libcommon"
version = ">=1.0.0"
```

## Installation Behavior

### Binary Installation

Binaries from `bin/` are installed to `/usr/local/bin/` with executable permissions:

1. Copy binary to `/usr/local/bin/<binary_name>`
2. Set executable permissions (`0o755`)
3. No conflicts with existing files allowed (overwrite)

### Library Installation

Libraries from `lib/` are installed to `/usr/local/lib/`:

1. Copy library to `/usr/local/lib/<lib_name>`
2. No conflicts allowed (overwrite)

### Registry Management

Installed packages are tracked in `/var/scpm/registry.toml`:

```toml
[[installed]]
name = "hello"
version = "1.0.0"
installed_at = 2024-01-01T00:00:00Z
files = ["/usr/local/bin/hello"]
```

## Repository Format

Packages can be indexed in a repository at `/var/scpm/repository/`:

```toml
[[packages]]
name = "hello"
version = "1.0.0"
description = "Hello World example"
architecture = "riscv64"
url = "file:///packages/hello-1.0.0.scarlet"
sha256 = "abc123..."
```

## Examples

### Minimal Package

```toml
# package.toml
name = "hello"
version = "1.0.0"
description = "Hello World example"
architecture = "any"
binaries = ["hello"]
dependencies = []
```

### Package with Dependencies

```toml
# package.toml
name = "editor"
version = "1.0.0"
description = "Text editor application"
architecture = "riscv64"
binaries = ["editor"]
dependencies = [
    { name = "libtext", version = ">=1.0.0" },
]
```

### Package with Libraries

```toml
# package.toml
name = "libcodec"
version = "1.0.0"
description = "Codec library"
architecture = "any"
libraries = ["libcodec.so"]
binaries = []
```

## Architecture Values

| Value | Description |
|--------|-------------|
| `riscv64` | RISC-V 64-bit architecture |
| `aarch64` | ARM 64-bit architecture |
| `any` | Architecture-independent (scripts, configs) |

## Naming Conventions

### Package Names
- Lowercase letters, numbers, and hyphens
- Must start with a letter
- Maximum length: 64 characters
- Examples: `hello`, `text-editor`, `network-utils`

### Versioning
- Follow Semantic Versioning (semver): `MAJOR.MINOR.PATCH`
- Examples: `1.0.0`, `0.5.1`, `2.1.3`
- Pre-release identifiers: `1.0.0-alpha`, `2.0.0-beta`

## Security Considerations

### Package Signing
- Currently not implemented (planned feature)
- SHA256 checksums recommended for verification

### Dependency Validation
- Dependency versions are constraints only (not strictly enforced in current implementation)
- Circular dependencies must be detected and prevented

## Compatibility Notes

### Scarlet OS Integration
- Uses `/var/scpm/` for data storage
- Binaries installed to `/usr/local/bin/`
- Libraries installed to `/usr/local/lib/`
- Environment variables not currently supported

### no_std Environment
- Packages must be compatible with Scarlet's `no_std` environment
- Dynamic linking is supported via `dlopen` (future feature)
- Static linking preferred for now
