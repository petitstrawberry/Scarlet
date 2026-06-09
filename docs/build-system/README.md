# Scarlet Build System

## Overview

This section documents the older project-local kernel build adapter. The current
distro/image architecture is defined in
[`docs/architecture/distro-model.md`](../architecture/distro-model.md).

`scarlet-config.toml` is no longer the distro or image composition source of
truth. It remains as a pre-split adapter input for project-local kernel builds.
Userland image composition lives in layer metadata under `layers/*/images/` and
is resolved through `cargo-scarlet plan` / `cargo-scarlet image --machine ...
--distro ... --image ...`; boot packaging is selected with `--boot ...` or the
distro default.

The intended end-state is that a **project is the main user-facing Scarlet project**. The current repository layout is only the in-tree development baseline used to prototype that model.

The core design goals are:

- keep the checked-in projects stable and small
- keep the kernel crate as the stable core crate
- treat drivers, ABI layers, filesystems, and similar pieces as real reusable crates
- generate the module selection layer from configuration instead of editing manifests by hand
- keep generated state local to the project instead of scattering it across the repository

## Current Repository Baseline

Today, this repository builds through `cargo make` from the repository root.

- `projects/riscv64-limine-full/Cargo.toml` and `projects/aarch64-limine-full/Cargo.toml` are the top-level kernel binaries
- both projects currently depend directly on `../../kernel`
- the kernel crate is still the canonical `scarlet` crate
- the kernel already uses linker-section-based initcalls (`.initcall.early`, `.initcall.driver`, `.initcall.late`)

That means the new build system should extend the existing flow instead of replacing the kernel/project boot contract.

However, the target developer experience is project-rooted:

- the **project becomes the main project root**
- `scarlet-config.toml` lives in that project
- the kernel crate and module crates are consumed as dependencies by that project
- user-facing usage can assume registry/online distribution as a normal case
- this development repository still uses local-path dependencies during implementation
- `.scarlet/scarlet-modules` is generated inside that project

## Documents

- [Project-Local Generated Modules Architecture](./project-local-generated-modules.md)
- [`scarlet-config.toml` Specification](./scarlet-config-spec.md)
- [Implementation Plan](./implementation-plan.md)
- [`cargo-scarlet` Prototype](./cargo-scarlet-prototype.md)

## Design Summary

The preferred direction is:

1. keep project manifests checked in and mostly static
2. treat the project as the main project root
3. add a static path dependency from the project to a project-local generated module crate
4. generate that crate from `scarlet-config.toml` before any Cargo operation
5. keep generated files inside the project directory under `.scarlet/`
6. keep the kernel crate focused on core kernel responsibilities

In short:

```text
project (main project root)
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
