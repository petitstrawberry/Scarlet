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
│  └─ modules/
│     └─ scarlet-module-prototype/
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

### Existing `cargo make` integration

The root `Makefile.toml` now includes generator pre-steps for BSP build/clippy tasks.

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
"scarlet-module-prototype" = { path = "../modules/scarlet-module-prototype", enabled = true }
```

### Generated output

The generated `scarlet-modules` crate:

- depends on all enabled module entries
- emits a `force_link()` function
- calls `module_crate::force_link()` for each enabled module

### BSP integration

Each BSP now has:

- a static dependency on `.scarlet/scarlet-modules`
- a call to `scarlet_modules::force_link()` in `arch_start_kernel()`

This keeps the generated module aggregation crate on a guaranteed-linked path without changing the kernel boot contract.

## Current Limitations

This is a prototype, not the final product.

Current intentional limitations:

- registry/catalog semantics are not fully implemented yet
- conflict/requirement logic is documented but not fully enforced in code yet
- the generator currently trusts dependency-style module entries directly
- the prototype currently ships with a tiny local sample module crate under `bsp/modules/`

## Intended Next Steps

1. move more validation logic from docs into the tool
2. add registry/catalog-backed conflict and requirement resolution
3. add nicer diagnostics and richer CLI commands
4. publish `cargo-scarlet` as a standalone external tool
