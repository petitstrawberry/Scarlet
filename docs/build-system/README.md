# Scarlet Build System

## Overview

This section documents the direction for Scarlet's next-generation build system.

The intended end-state is that a **BSP project is the main user-facing Scarlet project**. The current repository layout is only the in-tree development baseline used to prototype that model.

The core design goals are:

- keep the checked-in BSP projects stable and small
- keep the kernel crate as the stable core crate
- treat drivers, ABI layers, filesystems, and similar pieces as real reusable crates
- generate the module selection layer from configuration instead of editing manifests by hand
- keep generated state local to the BSP project instead of scattering it across the repository

## Current Repository Baseline

Today, this repository builds through `cargo make` from the repository root.

- `bsp/riscv64-limine/Cargo.toml` and `bsp/aarch64-limine/Cargo.toml` are the top-level kernel binaries
- both BSPs currently depend directly on `../../kernel`
- the kernel crate is still the canonical `scarlet` crate
- the kernel already uses linker-section-based initcalls (`.initcall.early`, `.initcall.driver`, `.initcall.late`)

That means the new build system should extend the existing flow instead of replacing the kernel/BSP boot contract.

However, the target developer experience is BSP-rooted:

- the **BSP project becomes the main project root**
- `scarlet-config.toml` lives in that BSP project
- the kernel crate and module crates are consumed as dependencies by that BSP project
- user-facing usage can assume registry/online distribution as a normal case
- this development repository still uses local-path dependencies during implementation
- `.scarlet/scarlet-modules` is generated inside that BSP project

## Documents

- [BSP-Local Generated Modules Architecture](./bsp-local-generated-modules.md)
- [`scarlet-config.toml` Specification](./scarlet-config-spec.md)
- [Implementation Plan](./implementation-plan.md)
- [`cargo-scarlet` Prototype](./cargo-scarlet-prototype.md)

## Design Summary

The preferred direction is:

1. keep BSP manifests checked in and mostly static
2. treat the BSP project as the main project root
3. add a static path dependency from the BSP project to a BSP-local generated module crate
4. generate that crate from `scarlet-config.toml` before any Cargo operation
5. keep generated files inside the BSP directory under `.scarlet/`
6. keep the kernel crate focused on core kernel responsibilities

In short:

```text
BSP project (main project root)
 ├─ scarlet-config.toml
 ├─ kernel crate (`scarlet`) from registry / git / path source
 └─ generated .scarlet/scarlet-modules
        ├─ selected driver crates
        ├─ selected ABI crates
        └─ selected filesystem / subsystem crates
```

This preserves the original philosophy:

- **OS as a Library**: reusable kernel and module crates
- **Infrastructure as a Tool**: configuration resolution lives in the build tool
- **Full-Config Principle**: `scarlet-config.toml` is a full resolved `.config`-style file, not a mini package manifest

In practice, module entries should still feel familiar: the preferred shape is a single `[modules]` table that reuses normal Cargo dependency syntax and adds an explicit `enabled = true/false` state.
