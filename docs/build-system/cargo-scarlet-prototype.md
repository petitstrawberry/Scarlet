# `cargo-scarlet` Prototype

## Overview

This repository now contains a prototype `cargo-scarlet` build tool as a standalone binary crate at:

```text
cargo-scarlet/
```

The prototype is intentionally simple:

- it lives at the repository top level
- it is a plain binary crate for now
- it is intended to become publishable later (for example to `crates.io`)
- it operates on BSP projects under `bsp/`
- the in-tree `bsp/` projects serve as the current sample/reference projects

## Prototype Layout

```text
Scarlet/
├─ cargo-scarlet/
│  ├─ Cargo.toml
│  └─ src/main.rs
├─ bsp/
│  ├─ riscv64-limine/
│  │  ├─ Cargo.toml
│  │  ├─ scarlet-config.toml
│  │  ├─ src/main.rs
│  │  └─ .scarlet/
│  │     └─ scarlet-modules/
│  ├─ aarch64-limine/
│  │  ├─ Cargo.toml
│  │  ├─ scarlet-config.toml
│  │  ├─ src/main.rs
│  │  └─ .scarlet/
│  │     └─ scarlet-modules/
├─ modules/
│  └─ scarlet-module-prototype/
├─ scripts/
│  └─ cargo-scarlet
```

## Commands

### Generate BSP-local module crate

```bash
cargo run --manifest-path cargo-scarlet/Cargo.toml -- generate --project bsp/riscv64-limine
```

This reads:

```text
bsp/riscv64-limine/scarlet-config.toml
```

and generates:

```text
bsp/riscv64-limine/.scarlet/scarlet-modules/Cargo.toml
bsp/riscv64-limine/.scarlet/scarlet-modules/src/lib.rs
```

### Build through the prototype tool

```bash
cargo run --manifest-path cargo-scarlet/Cargo.toml -- build --project bsp/riscv64-limine
```

This prototype currently does:

1. generate `.scarlet/scarlet-modules`
2. run `cargo metadata`
3. run `cargo build` for the BSP project

### Check through the prototype tool

```bash
cargo run --manifest-path cargo-scarlet/Cargo.toml -- check --project bsp/riscv64-limine
```

### Clippy through the prototype tool

```bash
cargo run --manifest-path cargo-scarlet/Cargo.toml -- clippy --project bsp/riscv64-limine
```

By default this runs clippy with `-D warnings`.

### Run through the prototype tool

```bash
cargo run --manifest-path cargo-scarlet/Cargo.toml -- run --project bsp/riscv64-limine --release
```

### Initialize Scarlet build-system files inside an existing project

```bash
cargo run --manifest-path cargo-scarlet/Cargo.toml -- init --project /tmp/my-board-project --board riscv64-limine
```

You can also override the prototype local paths explicitly:

```bash
cargo run --manifest-path cargo-scarlet/Cargo.toml -- init \
  --project /tmp/my-board-project \
  --board riscv64-limine \
  --kernel-path ../../kernel \
  --module-path ../../modules/scarlet-module-prototype
```

This now assumes `/tmp/my-board-project` already exists as a board/BSP project.

`init` adds or updates only the Scarlet build-system support files:

- `scarlet-config.toml`
- `.cargo/config.toml`
- `.gitignore` entries for `.scarlet` and `target`

It does **not** generate the board-specific project template itself.
Board templates are expected to come from a separately managed repository or other project scaffolding flow.

### Repository-local wrapper for future `cargo scarlet` usage

```bash
scripts/cargo-scarlet generate --project bsp/riscv64-limine
scripts/cargo-scarlet check --project bsp/riscv64-limine
```

This wrapper is a local convenience shim before `cargo-scarlet` is published and installed as a real Cargo subcommand.

### Existing `cargo make` integration

The root `Makefile.toml` now routes BSP build/clippy/run tasks through `cargo-scarlet` so generation and metadata validation stay on one path.

So existing flows like these also trigger generation first:

```bash
cargo make build-kernel-debug-riscv64
cargo make build-kernel-debug-aarch64
cargo make clippy-bsp-riscv64
cargo make clippy-bsp-aarch64
```

## Prototype Behavior

### Config input

The prototype reads dependency-like module entries from `[modules]` in `scarlet-config.toml`.

Example:

```toml
[modules]
"scarlet-module-prototype" = { path = "../../modules/scarlet-module-prototype", enabled = true }
```

### Generated output

The generated `scarlet-modules` crate:

- owns the configured `scarlet` kernel dependency and kernel feature set
- re-exports that kernel as `scarlet_modules::scarlet`
- depends on all enabled module entries
- emits a `force_link()` function
- calls `module_crate::force_link()` for each enabled module

### BSP integration

Each BSP now has:

- a static dependency on `.scarlet/scarlet-modules`
- a call to `scarlet_modules::force_link()` in `arch_start_kernel()`
- kernel entry access through `scarlet_modules::scarlet`

This keeps the generated module aggregation crate on a guaranteed-linked path without changing the kernel boot contract.

## Current Limitations

This is a prototype, not the final product.

Current intentional limitations:

- the generator currently trusts dependency-style module entries directly
- the prototype currently ships with a tiny local sample module crate under top-level `modules/`
- advanced semantic validation is intentionally deferred; the MVP relies on `cargo metadata` for real dependency/feature resolution checks

## Intended Next Steps

1. move more validation logic from docs into the tool
2. add nicer diagnostics and richer CLI commands
3. publish `cargo-scarlet` as a standalone external tool
