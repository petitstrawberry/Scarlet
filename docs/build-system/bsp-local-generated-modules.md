# BSP-Local Generated Modules Architecture

## Overview

This document describes the preferred Scarlet build-system direction for configurable kernel composition without per-build manifest editing.

The intended product model is that the **BSP project itself is the main Scarlet project**. Users should think in terms of a BSP-rooted project that consumes Scarlet kernel and module crates, not in terms of a giant monorepo with BSPs hidden inside it.

That includes the kernel itself: the BSP project should be able to consume the kernel core from a registry/online source in the normal user-facing case, while still supporting local-tree development in the Scarlet repository.

The design keeps:

- the kernel as a stable core crate
- BSP projects as the main user-facing project root and thin boot wrapper
- drivers, ABI implementations, filesystems, and similar subsystems as real reusable crates

The only generated part is the module aggregation crate, which is produced from `scarlet-config.toml` by the build tool.

## Problem Statement

Scarlet needs a way to select drivers and subsystems per target without forcing developers to repeatedly edit each BSP project's `Cargo.toml`.

The design must avoid these failure modes:

- manually toggling dependencies in each BSP manifest
- making the kernel crate aware of every possible external module
- turning module selection into implicit Cargo feature state spread across the tree
- relying on `target/` as a stable source directory

## Current Repository Baseline

At the time of writing, the in-tree Scarlet repository looks like this:

- `kernel/Cargo.toml` defines the main kernel crate with package name `scarlet`
- `bsp/riscv64-limine/Cargo.toml` and `bsp/aarch64-limine/Cargo.toml` are the final binary crates
- both BSPs currently depend on `scarlet = { path = "../../kernel" }`
- `Makefile.toml` invokes Cargo from inside each BSP directory for build, run, and clippy flows
- the kernel boot path already executes initcalls through:
  - `early_initcall_call()`
  - `driver_initcall_call()`
  - `call_initcalls()`
- linker scripts already collect `.initcall.early`, `.initcall.driver`, and `.initcall.late`

Because that boot contract already exists, the build system should preserve it.

That repository shape is only the current implementation baseline.
The intended external usage model is instead:

```text
my-board-project/
├─ scarlet-config.toml
├─ Cargo.toml
├─ src/main.rs
└─ .scarlet/
   └─ scarlet-modules/
```

with `scarlet` and real module crates supplied as dependencies.

Those dependencies may come from:

- a registry-compatible source
- a local path
- a vendored checkout
- a git repository

For the intended user-facing model, registry/online acquisition is a normal assumption.
For the current in-tree Scarlet development repository, local paths remain the practical development baseline.

## Design Goals

1. **No per-build BSP manifest editing**
2. **No generated BSP project replacement**
3. **Kernel remains the stable core crate**
4. **Module aggregation is ephemeral and config-derived**
5. **Generated state stays local to the BSP project**
6. **The design remains compatible with Cargo and rust-analyzer**

## Proposed Structure

Each BSP project keeps a checked-in dependency on a generated crate inside its own directory.

Example layout:

```text
my-board-project/
├─ scarlet-config.toml
├─ Cargo.toml
├─ src/main.rs
└─ .scarlet/
   └─ scarlet-modules/
      ├─ Cargo.toml
      └─ src/lib.rs
```

The current in-tree `bsp/riscv64-limine/` and `bsp/aarch64-limine/` directories are repository-local prototypes of that future BSP project shape.

## BSP Manifest Contract

The BSP project's manifest stays stable and declares two direct dependencies:

```toml
[dependencies]
scarlet = { path = "../vendor/scarlet/kernel" }
scarlet_modules = { package = "scarlet-modules", path = ".scarlet/scarlet-modules" }
```

This example uses a local vendored kernel checkout because the current repository is still a development tree. The architecture does **not** require the kernel to be local-only. In the user-facing model, the kernel source should be treated the same way the build system treats modules: a resolvable dependency source chosen by configuration and tooling.

This is a one-time structural dependency.

After that, module selection changes happen only through config regeneration.

In the current in-tree repository prototype, the `scarlet` path is `../../kernel` instead. The important point is not the literal path string; the important point is that the BSP project has one stable dependency on the kernel core crate and one stable dependency on the generated module aggregation crate.

Long-term, the build tool should be able to materialize or resolve the kernel dependency from whichever source the configuration specifies.

## Generated Crate Contract

The build tool reads the BSP project's `scarlet-config.toml` and generates `.scarlet/scarlet-modules/` before any Cargo command runs.

Crucially, `scarlet-config.toml` should be treated as a **full resolved `.config`-style file**.

In the MVP:

- the config records the explicit resolved state of all available options
- the config records module source provenance inline using dependency-like entries under `[modules]`
- the build tool generates the crate directly from those entries
- contradictions and dependency breakage are checked by `cargo metadata`

The generated crate is responsible for:

- depending on all selected module crates
- providing a stable symbol such as `force_link()`
- forcing selected modules to remain linked into the final image

Example generated manifest:

```toml
[package]
name = "scarlet-modules"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
scarlet-driver-pl011 = { version = "0.1.0" }
scarlet-abi-linux = { version = "0.1.0" }
my-custom-driver = { path = "../../../drivers/my-custom-driver" }
```

Those dependency entries come directly from the dependency-like entries in `scarlet-config.toml`.

Example generated library:

```rust
#![no_std]

pub fn force_link() {
    scarlet_driver_pl011::force_link();
    scarlet_abi_linux::force_link();
    my_custom_driver::force_link();
}
```

## BSP Runtime Anchor

The BSP should reference the generated crate from a code path that is always linked.

Conceptually:

```rust
extern crate scarlet;
extern crate scarlet_modules;

pub extern "C" fn arch_start_kernel() -> ! {
    scarlet_modules::force_link();
    scarlet::arch::riscv64::boot::limine::limine_entry()
}
```

The exact architecture-specific handoff differs per BSP, but the role is the same:

- keep the generated module crate reachable
- ensure selected modules are not dropped by dead-code elimination
- preserve the existing BSP-to-kernel boot contract

## Module Crate Contract

Real modules remain real crates.

Examples:

- `scarlet-driver-pl011`
- `scarlet-driver-virtio-blk`
- `scarlet-abi-linux`
- `scarlet-fs-ext2`

Each module crate should expose a small anchor such as:

```rust
pub fn force_link() {}
```

That anchor must live in the same object/module region that also contributes the module's initcall registrations or other retained linker data.

The purpose is not runtime registration by itself. The purpose is to make Cargo and the linker retain the code/object that contains the module's initialization hooks.

## How Initcalls Fit

Scarlet already has an initcall model, and the generated module architecture should use it instead of inventing a second lifecycle system.

Flow:

1. module crate contributes initcall entries
2. generated `scarlet-modules` crate references the module via `force_link()`
3. BSP references `scarlet_modules::force_link()` from a guaranteed path
4. final image retains the module objects
5. kernel boot continues to execute initcalls using the existing initcall runners

So the generated module crate is a **link-time aggregation layer**, not a new runtime registry.

## Why the Generated Crate Lives Under `.scarlet/`

The generated crate should stay inside the BSP directory, but it should not live under `target/`.

Recommended location:

```text
<bsp-project-root>/.scarlet/scarlet-modules/
```

Reasons:

- keeps generated state scoped to the BSP project
- avoids polluting the repository root
- avoids using Cargo's disposable build-output directory as a source input
- survives `cargo clean`
- gives rust-analyzer a stable path once generated

Using `target/scarlet-modules` is intentionally avoided because:

- `cargo clean` removes `target/`
- `CARGO_TARGET_DIR` changes make the path ambiguous
- IDEs and `cargo metadata` need the dependency to exist before building

## Build Tool Responsibilities

The build tool is responsible for all config resolution.

Required behavior:

1. read `scarlet-config.toml`
2. resolve enabled module options from dependency-like `[modules]` entries into concrete dependency definitions
3. generate `.scarlet/scarlet-modules/Cargo.toml`
4. generate `.scarlet/scarlet-modules/src/lib.rs`
5. run `cargo metadata` against the generated BSP dependency graph and fail on invalid resolution
6. do this before any Cargo command that touches the BSP project

That includes:

- `build`
- `run`
- `clippy`
- `check`
- IDE bootstrap / metadata-oriented commands

## Example Build Flow

```text
scarlet-config.toml
        │
        ▼
build tool resolves enabled modules
        │
        ▼
generate <bsp-project-root>/.scarlet/scarlet-modules/
        │
        ▼
validate generated graph with cargo metadata
        │
        ▼
run cargo from <bsp-project-root>/
        │
        ▼
BSP links kernel + generated scarlet-modules
        │
        ▼
selected module crates survive linking
        │
        ▼
kernel executes existing initcall pipeline
```

## Dependency Graph

```text
BSP binary crate
 ├─ scarlet                # kernel core crate
 └─ scarlet_modules        # generated BSP-local crate
      ├─ selected driver crates
      ├─ selected ABI crates
      ├─ selected filesystem crates
      └─ selected subsystem crates
```

## Why This Matches the Original Philosophy

This design keeps the architectural roles clean.

- **kernel** is the reusable core
- **drivers / ABIs / filesystems** are the reusable components
- **BSP** is the thin board-specific boot wrapper
- **generated `scarlet-modules`** is only an ephemeral build artifact
- **the build tool** is the place where config becomes dependency composition

That means module selection is not embedded into the kernel crate and not manually maintained in every BSP manifest.

## Operational Notes

- `.scarlet/` should be gitignored
- generation should be deterministic so rebuilds stay predictable
- the generated crate should not contain hidden defaults not represented in `scarlet-config.toml`
- if direct `cargo build` inside a BSP is expected to work, developers need a generation/bootstrap step first

## Non-Goals

This design does **not** aim to:

- support runtime module loading
- replace the current initcall mechanism
- make the kernel crate enumerate all possible modules directly
- use `target/` as a canonical source tree

## Summary

The BSP-local generated modules design gives Scarlet a Cargo-compatible way to compose the kernel from reusable crates without regenerating the BSP itself and without repeatedly editing BSP manifests.

In one sentence:

> keep the BSP fixed, keep the kernel core stable, and let the build tool synthesize a BSP-local module aggregation crate from `scarlet-config.toml`.
