# Package Format Specification

## Overview

SCPM packages are distributed as `.scarlet` archive files (tar.gz) containing package metadata and payload files.

## Archive Structure

A `.scarlet` file is a **tar.gz** archive with the following structure:

```
hello-1.0.0.scarlet  (tar.gz)
├── package.toml          # Required: Package metadata
├── bin/                  # Optional: Executable binaries
│   └── hello
├── lib/                  # Optional: Shared libraries
│   └── libhello.so
└── ...                   # Other files (config, data, etc.)
```

### File Installation Mapping

During installation, files under `scarlet/` directory prefix are extracted to the system root:

```
scarlet/                  # Package internal prefix
├── bin/hello            → /bin/hello
├── lib/libhello.so      → /lib/libhello.so
├── etc/config.conf      → /etc/config.conf
└── usr/share/data/      → /usr/share/data/
```

**Note**: Currently, the `scarlet/` prefix is optional. The package extracts files matching the prefix to system root. Files without the prefix are also extracted relative to root.

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

### File Extraction

1. **Archive Parsing**: The `.scarlet` file (tar.gz) is decompressed and parsed
2. **Metadata Reading**: `package.toml` is automatically parsed during extraction
3. **File Deployment**: Files are extracted from `scarlet/` prefix to system root
4. **Tracking**: All installed files are recorded in `PackageMetadata.installed_files`

### Safety Features

- **Overwrite Prevention**: Installation fails if target file already exists
- **Permission Preservation**: File modes from tar header are preserved
- **Atomic Installation**: Files are tracked for complete removal

### Example Installation Flow

```
1. Read hello-1.0.0.scarlet
2. Decompress gzip → tar
3. Parse tar entries:
   - package.toml → metadata
   - scarlet/bin/hello → /bin/hello
   - scarlet/lib/libhello.so → /lib/libhello.so
4. Record installed files:
   installed_files = ["/bin/hello", "/lib/libhello.so"]
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
```

### Package with Configuration

```
myapp-1.0.0.scarlet
├── package.toml
├── scarlet/
│   ├── bin/myapp
│   └── etc/myapp/config.conf
```

```toml
# package.toml
name = "myapp"
version = "1.0.0"
description = "My application"
architecture = "riscv64"
binaries = ["myapp"]
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

### Package Verification
- SHA256 checksums recommended
- Package signing (planned)

### Safety
- Overwrite prevention by default
- File ownership tracking
- Complete removal on uninstall

## Technical Details

### Archive Processing

SCPM uses these libraries for archive handling:
- **miniz_oxide**: gzip decompression
- **tar-no-std**: tar archive parsing

### File Type Detection

From tar header typeflag:
- Regular file (`REGTYPE`, `AREGTYPE`) → Installed as-is
- Directory (`DIRTYPE`) → Created with `fs::create_directory`
- Symlink (`SYMTYPE`) → Created with `fs::create_symlink`

### Metadata Parsing

Custom TOML parser (no serde):
- Line-by-line parsing
- Section support (`[package]`, `[bin]`)
- Simple key-value pairs
- Array parsing for binaries/libraries
