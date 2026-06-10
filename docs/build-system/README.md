# Scarlet Build System

## Overview

Scarlet uses a project-rooted build system centered on `scarlet.toml` manifests and the `cargo-scarlet` build tool.

Each project under `projects/` is an independent kernel build target with its own `scarlet.toml`. The build tool reads this manifest, generates a module aggregation crate under `.scarlet/`, and invokes Cargo for the project.

## Quick Start

```bash
# Build (via cargo make, which delegates to cargo-scarlet)
cargo make build-riscv64

# Run
cargo make run-riscv64
```

## Projects

```
projects/
├── riscv64-limine-full/       # Full RISC-V 64-bit (primary)
├── riscv64-limine-desktop/    # RISC-V desktop variant
├── aarch64-limine-full/       # Full AArch64
├── aarch64-limine-desktop/    # AArch64 desktop variant
├── aarch64-limine-microvm/    # AArch64 microvm (minimal)
└── aarch64-apple-limine-full/ # AArch64 Apple Silicon
```

Each project directory contains:
- `scarlet.toml` — Build manifest
- `src/main.rs` — Boot entry point
- `Cargo.toml` — Cargo manifest (auto-managed)
- `.scarlet/` — Generated module crate and build artifacts

## scarlet.toml Format

Current schema version: **1**

```toml
schema_version = 1

[project]
name = "scarlet-riscv64-limine-full"

[kernel]
package = "scarlet"
source = "../../kernel"
target = "riscv64gc-unknown-none-elf"
target_json = "../../kernel/targets/riscv64gc-unknown-none-elf.json"

[kernel.features]
network = true
user-fpu = true
user-vector = true
hypervisor = true
limine = true

[modules]
"scarlet-module-prototype" = { path = "../../modules/scarlet-module-prototype", enabled = true }

[images]

[images.initramfs]
format = "newc"
output = ".scarlet/images/initramfs-riscv64-full.cpio"

[[images.initramfs.layers]]
path = "../../bundles/base/bundle.toml"

[images.rootfs]
format = "ext2"
output = ".scarlet/images/rootfs-riscv64-full.ext2"

[[images.rootfs.layers]]
path = "../../bundles/full/bundle.toml"

[images.boot]
format = "limine-uefi"
output = ".scarlet/images/limine-riscv64-full.img"
cmdline = "console=ttyS0 root=/dev/vblk1"
deps = ["initramfs"]

[[images.boot.packages]]
source = ".scarlet/images/initramfs-riscv64-full.cpio"
to = "/boot/initramfs"

[runner]
command = "tools/run.sh"
```

### Top-Level Sections

| Section | Purpose |
|---------|---------|
| `[project]` | Project name |
| `[kernel]` | Kernel crate source, target, features |
| `[modules]` | External module crates (path, git, registry) |
| `[images]` | Image composition (initramfs, rootfs, boot) |
| `[runner]` | QEMU run command |

### Kernel Section

| Field | Description |
|-------|-------------|
| `package` | Kernel crate name |
| `source` | Path to kernel crate |
| `target` | Rust target triple |
| `target_json` | Custom target JSON file |
| `features` | Feature flags (key = name, value = bool) |

### Modules Section

Module entries support Cargo-like dependency sources:

```toml
[modules]
# Path dependency
"my-module" = { path = "../../modules/my-module", enabled = true }
# Git dependency
"other-module" = { git = "https://github.com/...", rev = "abc123", enabled = true }
# Registry dependency
"published-module" = { version = "0.1.0", enabled = true }
# With features
"mod-with-features" = { path = "../mod", features = ["foo"], enabled = true }
```

### Images Section

Images are composed from layers (bundle TOML files). Each image has a format and output path:

| Format | Description |
|--------|-------------|
| `newc` | CPIO newc format (initramfs) |
| `ext2` | ext2 filesystem (rootfs) |
| `limine-uefi` | Limine UEFI boot image |

Boot images can declare `deps` on other images and include `packages` that copy artifacts into the boot image.

## cargo-scarlet

`cargo-scarlet` is the build tool located at `cargo-scarlet/`. It is a Cargo subcommand that:

1. Reads `scarlet.toml` from a project directory
2. Generates `.scarlet/scarlet-modules/` crate with selected modules
3. Invokes Cargo to build the project kernel binary
4. Composes images (initramfs, rootfs, boot) from bundle layers

### Subcommands

| Command | Description |
|---------|-------------|
| `build` | Build kernel binary |
| `check` | Type-check without building |
| `clippy` | Run clippy on the project |
| `run` | Build and run in QEMU |
| `image` | Build images only |
| `new` | Create new project or module |
| `lock` | Lock image layer hashes |

### Usage

```bash
# Via cargo-scarlet directly
cargo scarlet build --project projects/riscv64-limine-full
cargo scarlet run --project projects/riscv64-limine-full --release

# Via cargo make (wraps cargo-scarlet)
cargo make build-riscv64
cargo make run-riscv64
```

## Repository Build Flow

```
cargo make build-riscv64
    └─> cargo scarlet build --project projects/riscv64-limine-full
         ├─> Read scarlet.toml
         ├─> Generate .scarlet/scarlet-modules/
         ├─> cargo build --target riscv64gc-unknown-none-elf
         └─> Compose images (initramfs, rootfs, boot)
```

## See Also

- [Distribution Model](../architecture/distro-model.md)
