# Scarlet Build System

## Overview

Scarlet uses a project-rooted build system centered on `scarlet.toml` manifests and the `cargo-scarlet` CLI tool.

Each project is an independent kernel build target with its own `scarlet.toml`. The build tool reads the manifest, generates a module aggregation crate under `<project>/.scarlet/`, invokes Cargo for the kernel build, and composes bootable images from ordered layers.

## Quick Start

```bash
# Build kernel + compose all images
cargo scarlet image --project projects/riscv64-limine-full

# Build and run
cargo scarlet run --project projects/riscv64-limine-full --release

# Equivalent via cargo make
cargo make build-riscv64
cargo make run-riscv64
```

## Cross C compiler selection

The Nix development shell and Docker image provide unwrapped Clang as
`TARGET_CC`, the cc-rs fallback for cross compilation. Native C builds keep
using `HOST_CC`/`CC` and the Nix compiler wrapper. Avoid exporting global
`CC_<target>` defaults: they override application settings in Cargo's `[env]`
table, including `yt-for-scarlet`'s cross GCC selection.

Applications can select a different cross compiler with `CC_<target>` in
their own `.cargo/config.toml`; cc-rs checks that before `TARGET_CC`. This
controls C headers and compilation only, not Rust `std` support. Run
`cargo make test-cross-cc` in the Nix shell to check both Scarlet targets,
application overrides, and native compiler selection. After updating the
flake, reload direnv or re-enter the Nix shell to discard old environment
variables.

## Project Layout

```
projects/
├── riscv64-limine-full/
│   ├── scarlet.toml              # Build manifest
│   ├── Cargo.toml                # Auto-managed by cargo-scarlet
│   ├── build.rs
│   ├── src/main.rs               # Boot entry point
│   ├── lds/                      # Linker scripts (BSP-managed)
│   ├── .cargo/config.toml        # Cargo build config (BSP-managed)
│   ├── .scarlet/
│   │   ├── scarlet-modules/      # Generated module aggregation crate
│   │   │   ├── Cargo.toml
│   │   │   ├── src/lib.rs
│   │   │   └── .cargo/config.toml # Cargo config (BSP-managed)
│   │   ├── images/               # Generated image artifacts
│   │   ├── staging/              # Temporary staging directories
│   │   └── cache/                # Git/URL fetch cache
│   └── scarlet.lock              # Pinned source revisions (auto-generated)
├── riscv64-limine-desktop/
├── aarch64-limine-full/
├── aarch64-limine-desktop/
├── aarch64-limine-microvm/
└── aarch64-apple-limine-full/
```

## CLI Reference

### Subcommands

| Command | Description |
|---------|-------------|
| `build` | Build kernel binary and inject kernel symbol table |
| `check` | Type-check without building |
| `clippy` | Run clippy on the project |
| `run` | Build images and launch via `[runner]` command |
| `image` | Build kernel + compose all images |
| `update` | Resolve git/URL sources and write `scarlet.lock` |
| `new --project` | Scaffold a new project |
| `new --lsm` | Scaffold a new loadable scarlet module (LSM) |

### Common Flags

| Flag | Applies to | Description |
|------|-----------|-------------|
| `--project <path>` | most commands | Project directory (defaults to current directory for `build`) |
| `--release` | build, check, clippy, run, image | Build in release mode |
| `--target <triple>` | build, check, clippy, run, image | Override kernel target |
| `--no-build` | image | Skip kernel build, only compose images |
| `--no-image` | run | Skip image composition |
| `--kernel-elf <path>` | image | Use a specific kernel ELF instead of building |
| `--lsm <path>` | build | Build a loadable scarlet module (LSM) instead of a project |

### Usage

```bash
# Build
cargo scarlet build --project projects/riscv64-limine-full
cargo scarlet build --project projects/riscv64-limine-full --release

# Check / Clippy
cargo scarlet check --project projects/riscv64-limine-full
cargo scarlet clippy --project projects/riscv64-limine-full

# Image composition
cargo scarlet image --project projects/riscv64-limine-full
cargo scarlet image --project projects/riscv64-limine-full --no-build

# Run
cargo scarlet run --project projects/riscv64-limine-full --release

# Resolve and pin sources
cargo scarlet update --project projects/riscv64-limine-full

# Scaffold
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf
cargo scarlet new --lsm my-module
```

## scarlet.toml Format

Schema version: **2**

### Full Example

```toml
schema_version = 2

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
kind = "bundle"
path = "../../bundles/base/bundle.toml"

[images.rootfs]
format = "ext2"
output = ".scarlet/images/rootfs-riscv64-full.ext2"

[[images.rootfs.layers]]
kind = "bundle"
path = "../../bundles/full/bundle.toml"

[images.boot]
format = "limine-uefi"
output = ".scarlet/images/limine-riscv64-full.img"
cmdline = "console=ttyS0 root=/dev/vblk1"
deps = ["initramfs"]

[[images.boot.layers]]
kind = "image"
source = "initramfs"
to = "/boot/initramfs"

[runner]
command = "tools/run.sh"
```

### Top-Level Sections

| Section | Required | Purpose |
|---------|----------|---------|
| `[project]` | yes | Project name |
| `[kernel]` | yes | Kernel crate source, target, features |
| `[modules]` | no | Static module crates (linked into kernel via `force_link`) |
| `[images]` | no | Image composition (initramfs, rootfs, boot) |
| `[runner]` | no | Run command (script or binary) |

### Kernel Section

| Field | Type | Description |
|-------|------|-------------|
| `package` | string | Kernel crate name in Cargo |
| `source` | string or table | Path or git source for the kernel crate |
| `target` | string | Rust target triple (e.g., `riscv64gc-unknown-none-elf`) |
| `target_json` | string | Path to custom target JSON file |
| `features` | table | Feature flags (key = name, value = bool) |

#### Kernel Source

The `source` field supports path or git references:

```toml
# Path source (local development)
source = "../../kernel"

# Git source
source = { git = "https://github.com/petitstrawberry/scarlet-kernel", branch = "main" }
source = { git = "https://github.com/petitstrawberry/scarlet-kernel", tag = "v0.17.0" }
source = { git = "https://github.com/petitstrawberry/scarlet-kernel", rev = "abc123def456" }
```

When using a git source, cargo-scarlet resolves the branch/tag to a commit revision and caches the checkout under `<project>/.scarlet/cache/git/`. The resolved revision is recorded in `scarlet.lock`.

### Modules Section (Static)

Modules listed in `[modules]` are statically linked into the kernel via the generated `scarlet-modules` crate. Each enabled module's `force_link()` is called at boot. These are regular Cargo crates — not loadable scarlet modules (LSMs).

Module entries support Cargo-like dependency sources:

```toml
[modules]
# Path dependency
"my-module" = { path = "../../modules/my-module", enabled = true }

# Git dependency
"other-module" = { git = "https://github.com/...", branch = "main", enabled = true }
"pinned-module" = { git = "https://github.com/...", rev = "abc123", enabled = true }

# Registry dependency
"published-module" = { version = "0.1.0", enabled = true }

# With features
"mod-with-features" = { path = "../mod", features = ["foo"], enabled = true }

# Disabled (included but not linked)
"optional-mod" = { path = "../opt", enabled = false }
```

### Images Section

Images are composed from ordered layers. Each image has a `format` and `output` path:

| Format | Description |
|--------|-------------|
| `newc` | CPIO newc format (initramfs) |
| `ext2` | ext2 filesystem (rootfs) |
| `limine-uefi` | Limine UEFI boot image (delegated to plugin) |

Images can declare:
- `deps` — list of other image names that must be built first (topological sort)
- `layers` — ordered filesystem/image composition operations
- `cmdline` — kernel command line (for boot images)

### Runner Section

```toml
[runner]
command = "tools/run.sh"
```

The runner command is executed from the project directory. When `--release` is passed, the environment variable `SCARLET_RELEASE=1` is set.

### scarlet.local.toml (Override)

A `scarlet.local.toml` file in the project directory is automatically merged on top of `scarlet.toml`. This allows per-developer overrides without modifying the tracked manifest:

```toml
# scarlet.local.toml
[kernel]
source = "../../my-fork/kernel"
```

The merge is deep: tables are merged recursively, arrays are appended. This file should be `.gitignore`d.

## Layers

Layers are applied in declaration order. Later layers can overwrite files created by earlier layers.

### Bundle (`kind = "bundle"`)

Expand another TOML file's `[[layers]]` at this exact position:

```toml
[[images.rootfs.layers]]
kind = "bundle"
path = "../../bundles/full/bundle.toml"
```

### Copy (`kind = "copy"`)

Copy files or directories into the image:

```toml
[[images.rootfs.layers]]
kind = "copy"
source = "rootfs"
to = "/"
```

### Cargo (`kind = "cargo"`)

Build a binary from a Cargo package and install it:

```toml
[[images.initramfs.layers]]
kind = "cargo"
source = "../../user/bin"
package = "user-bin"
bin = "sh"
to = "/system/scarlet/bin/sh"
```

For a package below the root of a Git checkout, set a relative `subdir`:

```toml
[[layers]]
kind = "cargo"
source = { git = "https://github.com/petitstrawberry/scarlet-ui" }
subdir = "examples/widget-factory"
package = "scarlet-ui-widget-factory"
bin = "scarlet-ui-widget-factory"
to = "/system/scarlet/bin/widget-factory"
```

Cargo layers can also control Cargo feature selection for that one installed
binary:

```toml
[[layers]]
kind = "cargo"
source = "../../user/video_player"
package = "video_player"
bin = "video-player"
default-features = false
features = ["av1-stateful-hw", "h264-stateful-hw", "mp4-aac"]
to = "/system/scarlet/bin/video-player"
```

| Field | Description |
|-------|-------------|
| `kind` | Must be `"cargo"` |
| `source` | Path or git source of the Cargo workspace |
| `subdir` | Optional package/workspace directory relative to the source root |
| `package` | Cargo package name |
| `bin` | Binary target name |
| `default-features` | Optional Cargo default feature switch |
| `features` | Optional Cargo feature list |
| `replace` | Replace earlier cargo layers with the same `to` path |
| `to` | Install path inside the image |

### Script (`kind = "script"`)

Run a script and install its output:

```toml
[[images.rootfs.layers]]
kind = "script"
source = "tools/fetch_skk_dictionary.sh"
output = ".scarlet/cache/skk/SKK-JISYO.L"
to = "/system/scarlet/share/skk/SKK-JISYO.L"
```

### Image (`kind = "image"`)

Reference another image's output as input:

```toml
[[images.boot.layers]]
kind = "image"
source = "initramfs"
to = "/boot/initramfs"
```

### Template Variables

Paths in `source`, `path`, `output`, and `to` support template expansion:

| Variable | Expands to |
|----------|-----------|
| `{target_triple}` | Kernel target triple (e.g., `riscv64gc-unknown-none-elf`) |
| `{arch}` | Architecture shorthand (e.g., `riscv64`, `aarch64`) |

### URL Sources

Copy layer sources can reference remote URLs. Fetched files are cached under `<project>/.scarlet/cache/files/` and verified against the lock file hash:

```toml
[[images.initramfs.layers]]
kind = "copy"
source = "https://example.com/firmware.bin"
to = "/lib/firmware/example.bin"
```

## Bundles (Layers)

Bundles are external TOML files containing ordered `[[layers]]` definitions. A `kind = "bundle"` layer expands a bundle in place:

```toml
# bundles/base/bundle.toml
[[layers]]
kind = "cargo"
source = "../../user/bin"
package = "user-bin"
bin = "sh"
to = "/system/scarlet/bin/sh"
```

```toml
# In scarlet.toml
[[images.initramfs.layers]]
kind = "bundle"
path = "../../bundles/base/bundle.toml"
```

All relative paths in a bundle are resolved from the bundle file's directory. Layers are applied in declaration order; later entries override earlier ones on conflict.

## Lock File (`scarlet.lock`)

The lock file pins exact source revisions for reproducible builds. It is auto-generated and should be committed to version control.

### Behavior

| Command | Network access | Reads lock | Writes lock |
|---------|---------------|------------|-------------|
| `build` | no | no | no |
| `image` | no | yes | yes (updates hash/git revs from lock) |
| `run` | no | yes | yes (via image) |
| `update` | **yes** | yes | **yes** |

- `update` is the only command that performs network access (git ls-remote, URL fetches)
- `image` reads the lock for cached git revisions but never resolves new ones
- If no lock exists, `image` resolves git sources from the manifest directly

### Format

```toml
# Generated by cargo-scarlet — do not edit

[sections.initramfs]
hash = "sha256:abc123..."

[[sections.initramfs.layers]]
kind = "copy"
source = "https://example.com/firmware.bin"
hash = "sha256:def456..."

[[sections.initramfs.layers]]
kind = "cargo"
git = "https://github.com/example/module"
resolved_rev = "abc123def456"
hash = "sha256:789..."

[sections.boot]
hash = "sha256:fed789..."
```

### Verification Rules

| Source type | Verification |
|------------|-------------|
| Local path | No hash verification (content changes during development) |
| Git (pinned rev) | No hash verification (rev is content-addressed) |
| URL fetch | Hash verified against lock; error on mismatch |

## Plugin System

Image formats that require external tools are handled via plugins. The core build tool resolves `format` to a plugin name and communicates via stdin JSON.

### Protocol

1. `cargo-scarlet` resolves `format` to plugin binary name: `cargo-scarlet-plugin-{format-prefix}`
2. Core builds a `PluginRequest` JSON and writes it to the plugin's stdin
3. Plugin reads the request, performs its work, and exits with code 0 on success

### PluginRequest Schema

```json
{
  "project_dir": "/path/to/project",
  "section_name": "boot",
  "format": "limine-uefi",
  "arch": "riscv64",
  "kernel_elf": "/path/to/kernel/elf",
  "initramfs": "/path/to/initramfs.cpio",
  "output": "/path/to/output.img",
  "section": {
    "cmdline": "console=ttyS0",
    "packages": [...]
  }
}
```

The `section` field is a plugin-facing compatibility view containing the data a plugin needs after ordered layers have been resolved.

### Plugin Discovery

Plugins are discovered via `PATH`. Install them with:

```bash
cargo install --path cargo-scarlet-plugin-limine
```

### Available Plugins

| Plugin | Format | Description |
|--------|--------|-------------|
| `cargo-scarlet-plugin-limine` | `limine-uefi` | Limine UEFI boot image generation |

## Generated File Structure

`cargo-scarlet` generates and manages files under `<project>/.scarlet/`:

```
.scarlet/
├── scarlet-modules/           # Module aggregation crate (auto-generated — do not edit)
│   ├── Cargo.toml             # Generated with kernel + module dependencies
│   ├── src/lib.rs             # Re-exports kernel + calls module force_link()
│   └── .cargo/config.toml     # Auto-generated template (do not edit)
├── images/                    # Generated image artifacts
├── staging/                   # Temporary staging for image composition
└── cache/
    ├── git/                   # Git checkout cache
    └── files/                 # URL fetch cache
```

All files under `.scarlet/` are auto-generated by cargo-scarlet. Do not edit them manually.

## Build Flow

```
cargo scarlet image --project projects/riscv64-limine-full
     │
     ├─ Read scarlet.toml + scarlet.local.toml
     ├─ Resolve ordered layers and expand bundle layers in place
     ├─ Generate .scarlet/scarlet-modules/
     │   ├─ Cargo.toml (kernel + module deps with correct source specs)
     │   ├─ src/lib.rs (force_link for enabled modules)
     │   └─ .cargo/config.toml (auto-generated template)
     ├─ cargo build --target <target_json>
     ├─ Inject .ksym section into kernel ELF
     ├─ Resolve git sources (from lock or fresh resolve)
     ├─ Topological sort images by deps
     └─ For each image:
         ├─ newc/ext2: apply ordered layers to staging → generate image
         └─ limine-uefi: run_plugin("limine", request) → generate image
```

## Scaffolding

### New Project

```bash
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf
```

Creates:
- `my-board/Cargo.toml`, `build.rs`, `src/main.rs`
- `my-board/scarlet.toml` (with kernel source pointing to `../../kernel`)
- `my-board/.cargo/config.toml` (with target, build-std)
- `my-board/.scarlet/scarlet-modules/` (initial generated crate)
- `my-board/lds/` (empty, for linker scripts)

With git source:
```bash
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf --kernel-rev v0.17.0
```

With explicit kernel path:
```bash
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf --kernel-path /path/to/kernel
```

Post-scaffold, the user must:
1. Edit `.cargo/config.toml` — set `build.target`, `unstable.build-std`, `runner`, and `rustflags`
2. Add a linker script to `lds/`
3. Implement the boot entry in `src/main.rs`

### New Loadable Scarlet Module (LSM)

```bash
cargo scarlet new --lsm my-module
```

Creates a loadable scarlet module with `Cargo.toml`, `module.toml`, `build.rs`, `src/lib.rs`, and `.cargo/config.toml`.

## See Also

- [Distribution Model](../architecture/distro-model.md)
