# scarlet-sdk

Build tools for [Scarlet](https://github.com/petitstrawberry/Scarlet).

## Installing

```bash
cargo install --git https://github.com/petitstrawberry/scarlet-sdk
```

Or from a local clone:

```bash
git clone https://github.com/petitstrawberry/scarlet-sdk.git
cd scarlet-sdk
cargo install --path cargo-scarlet
cargo install --path cargo-scarlet-plugin-limine
```

## Tools

| Package | Binary | Description |
|---------|--------|-------------|
| `cargo-scarlet` | `cargo-scarlet` | Build system CLI — reads `scarlet.toml`, builds kernel, composes images |
| `cargo-scarlet-plugin-limine` | `cargo-scarlet-plugin-limine` | Limine UEFI boot image plugin |

## Quick Start: Creating a Project

```bash
# Scaffold a new project with a local kernel source
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf --kernel-path /path/to/kernel

# Or with a git source (defaults to github.com/petitstrawberry/Scarlet)
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf --kernel-rev v0.17.0
```

This generates:

```
my-board/
├── Cargo.toml
├── build.rs
├── scarlet.toml
├── src/main.rs                           # TODO: implement arch_start_kernel
├── lds/                                  # TODO: add linker script
├── .cargo/config.toml                    # TODO: set runner, rustflags
└── .scarlet/scarlet-modules/
    ├── Cargo.toml
    ├── src/lib.rs
    └── .cargo/config.toml                # TODO: set target, build-std, rustflags
```

After scaffolding, you need to:

1. Edit `.cargo/config.toml` — set runner, rustflags (linker script path), etc.
2. Edit `.scarlet/scarlet-modules/.cargo/config.toml` — set `build.target`, `unstable.build-std`, and `rustflags` for the module crate
3. Add a linker script to `lds/`
4. Implement the boot entry in `src/main.rs` (e.g. call `scarlet::arch::riscv64::boot::limine::limine_entry()`)

Both `.cargo/config.toml` files are generated as commented templates and will not be overwritten on subsequent builds.

Then build and run:

```bash
cargo scarlet image --project my-board
cargo scarlet run --project my-board --release
```

## Quick Start: Creating a Module

```bash
cargo scarlet new --module my-module
```

This generates a loadable kernel module with `Cargo.toml`, `module.toml`, `build.rs`, and `src/lib.rs`.

```bash
# Build the module
cargo scarlet build --module my-module

# Enable it in your project's scarlet.toml
# [modules]
# "my-module" = { path = "../my-module", enabled = true }
```

## Commands

```bash
cargo scarlet build --project <path>              # Build kernel binary
cargo scarlet check --project <path>              # Type-check without building
cargo scarlet clippy --project <path>             # Run clippy
cargo scarlet image --project <path>              # Build kernel + compose images
cargo scarlet run --project <path> --release      # Build images and launch runner
cargo scarlet update --project <path>             # Resolve git/URL sources, write lock
cargo scarlet new --project <name> --target <triple>  # Scaffold new project
cargo scarlet new --module <name>                     # Scaffold new module
```

## Documentation

See [Scarlet Build System docs](https://github.com/petitstrawberry/Scarlet/tree/main/docs/build-system) for the full specification.
