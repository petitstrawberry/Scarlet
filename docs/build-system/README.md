# Scarlet Build System

## Overview

`scarlet.toml` is the project-local kernel build configuration. It declares the
kernel source, target, features, module selection, and image composition for a
specific project.

The distro/image architecture is described in
[`docs/architecture/distro-model.md`](../architecture/distro-model.md).

The core design goals are:

- keep the checked-in projects stable and small
- keep the kernel crate as the stable core crate
- treat drivers, ABI layers, filesystems, and similar pieces as real reusable crates
- generate the module selection layer from configuration instead of editing manifests by hand
- keep generated state local to the project instead of scattering it across the repository

## Repository Baseline

This repository builds through `cargo make` from the repository root, or through
`cargo scarlet` for project-centric workflows.

- `projects/riscv64-limine-full/`, `projects/aarch64-limine-full/`, `projects/aarch64-limine-microvm/`, and `projects/aarch64-apple-limine-full/` are the top-level kernel binaries
- each project has a `scarlet.toml` manifest for kernel build configuration
- all projects depend on `../../kernel` via path dependencies
- `projects/aarch64-limine-microvm/` additionally uses `scarlet.toml` for image composition (initramfs, rootfs, boot image)
- the kernel crate is still the canonical `scarlet` crate
- the kernel already uses linker-section-based initcalls (`.initcall.early`, `.initcall.driver`, `.initcall.late`)

The developer experience is project-rooted:

- the **project is the main project root**
- `scarlet.toml` lives in that project
- the kernel crate and module crates are consumed as dependencies by that project
- `.scarlet/scarlet-modules` is generated inside that project

## Documents

- [`scarlet.toml` Specification](./scarlet-config-spec.md)
- [Implementation Plan](./implementation-plan.md)
- [`cargo-scarlet` Prototype](./cargo-scarlet-prototype.md)
- [Project-Local Generated Modules Architecture](./project-local-generated-modules.md)

## Design Summary

1. keep project manifests checked in and mostly static
2. treat the project as the main project root
3. add a static path dependency from the project to a project-local generated module crate
4. generate that crate from `scarlet.toml` before any Cargo operation
5. keep generated files inside the project directory under `.scarlet/`
6. keep the kernel crate focused on core kernel responsibilities

In short:

```text
project (main project root)
 ├─ scarlet.toml
 ├─ kernel crate (`scarlet`) from registry / git / path source
 └─ generated .scarlet/scarlet-modules
        ├─ selected driver crates
        ├─ selected ABI crates
        └─ selected filesystem / subsystem crates
```

This preserves the original philosophy:

- **OS as a Library**: reusable kernel and module crates
- **Infrastructure as a Tool**: configuration resolution lives in the build tool
- **Manifest Principle**: `scarlet.toml` declares kernel source, features, modules, and image composition

In practice, module entries should still feel familiar: the preferred shape is a single `[modules]` table that reuses normal Cargo dependency syntax and adds an explicit `enabled = true/false` state.
