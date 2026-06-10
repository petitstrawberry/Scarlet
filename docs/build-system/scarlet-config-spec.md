# `scarlet-config.toml` Specification

## Overview

`scarlet-config.toml` is the current project-local kernel build configuration.
It defines the target board, kernel crate source, feature flags, and module
selection for a specific project.

The planned evolution to a unified `scarlet.toml` with distro/image composition
is documented in
[`docs/architecture/distro-model.md`](../architecture/distro-model.md).
Until that model is implemented, `scarlet-config.toml` remains the active
configuration format.

It should be understood as a **full resolved `.config`-style file** in the Kconfig sense: every meaningful build option is written out explicitly, including options that are disabled.

Its job is to describe:

- which project target is being built
- which kernel crate/version is the core
- which kernel features are enabled or disabled
- which module options are enabled or disabled
- the resolved source choice for the kernel and module inputs

The build tool reads this file and generates the project-local `.scarlet/scarlet-modules` crate before invoking Cargo.

## Design Principles

The format follows these rules:

1. **Full-Config Principle** — all effective build choices should be visible in the file
2. **No hidden module selection** — enabled modules are declared explicitly
3. **Full enumeration** — disabled options stay visible instead of disappearing
4. **Kernel stays core** — the file composes around the kernel crate instead of replacing it
5. **Resolved source provenance** — local path, `crates.io`, and git sourcing should be explicit in the config when they affect the resolved build
6. **Cargo-shaped inputs** — modules are described directly with dependency-like entries in the config
7. **Tool-owned expansion** — the build tool turns those entries into generated Cargo manifests and lets Cargo validate the resolved graph

## File Location

Recommended location:

```text
<project-root>/scarlet-config.toml
```

In the intended model, `<project-root>` means the **project root**.

Example:

```text
my-board-project/
├─ scarlet-config.toml
├─ Cargo.toml
├─ src/main.rs
└─ .scarlet/
```

The current Scarlet repository may still keep prototype projects under `projects/`, but that is an implementation detail of the in-tree development environment, not the intended user-facing project model.

## Top-Level Structure

```toml
config_version = 1

[project]
name = "scarlet-qemu-riscv64"

[board]
name = "riscv64-limine-full"
target = "riscv64gc-unknown-none-elf"
target_json = "kernel/targets/riscv64gc-unknown-none-elf.json"

[kernel]
package = "scarlet"
source = { version = "0.16.0" }

[kernel.features]
network = true
user-fpu = true
user-vector = true
hypervisor = true
limine = true
profiler = false

[modules]
"scarlet-driver-pl011" = { version = "0.1.0", enabled = true }
"scarlet-abi-linux" = { path = "modules/abi/linux", enabled = true }
"community-net-stack" = { git = "https://github.com/org/net", rev = "abc123", enabled = false }
```

## `config_version`

```toml
config_version = 1
```

- Required
- Integer
- Used by the build tool to validate schema compatibility

If the schema changes incompatibly in the future, this number must increase.

## `[project]`

```toml
[project]
name = "scarlet-qemu-riscv64"
```

### Fields

- `name` — required string used as a human-readable project/configuration name

This field is for tooling, logs, generated metadata, and diagnostics. It does not need to match any Cargo package name.

## `[board]`

```toml
[board]
name = "riscv64-limine-full"
target = "riscv64gc-unknown-none-elf"
target_json = "kernel/targets/riscv64gc-unknown-none-elf.json"
```

### Fields

- `name` — required string identifying the board/project profile
- `target` — required Rust target triple or custom target base name
- `target_json` — required path to the target JSON used by Cargo

### Purpose

This section tells the build tool which project profile to prepare and which Cargo target configuration to invoke.

The build tool generates into the current project root:

```text
<project-root>/.scarlet/scarlet-modules/
```

## `[kernel]`

```toml
[kernel]
package = "scarlet"
source = { version = "0.16.0" }
```

### Fields

- `package` — required package/crate identity of the kernel core
- `source` — required inline table describing where the kernel crate comes from

### Rules

- `source` must use exactly one supported source form
- `package` should remain `scarlet` while the kernel crate is still the stable core crate

### Supported kernel source forms

The kernel should support the same broad acquisition modes as modules.

For the intended user-facing model, a registry-compatible version source is the simplest default example.
For the current in-tree Scarlet development repository, a local path source remains the normal prototype shape.

#### Registry-like source

```toml
[kernel]
package = "scarlet"
source = { version = "0.16.0" }
```

Use when the kernel is distributed through a registry-compatible packaging flow.

#### Local path source

```toml
[kernel]
package = "scarlet"
source = { path = "../vendor/scarlet/kernel" }
```

Use for vendored or checked-out local development.

#### Git source

```toml
[kernel]
package = "scarlet"
source = { git = "https://github.com/scarlet-os/scarlet", rev = "v0.16.0" }
```

Use when the project should fetch the kernel from an online repository.

The exact publication mechanics may evolve, but the config format should already permit this source form.

## `[kernel.features]`

```toml
[kernel.features]
network = true
user-fpu = true
user-vector = true
hypervisor = true
limine = true
profiler = false
```

### Purpose

This table explicitly captures kernel feature state.

### Rules

- every known build-relevant kernel feature should appear explicitly
- values are booleans
- omitted features should be treated as schema errors, not as silent defaults, once the format is stabilized

This section exists to prevent feature state from being hidden in ad hoc Cargo invocation flags.

## `[modules]`

`[modules]` should look like normal Cargo dependencies as much as possible.

Each key is a module option name, and each value is an inline table that uses ordinary dependency source fields plus an explicit `enabled` flag.

Example:

```toml
[modules]
"scarlet-driver-pl011" = { version = "0.1.0", enabled = true }
"scarlet-driver-ns16550" = { version = "0.2.0", enabled = false }
"scarlet-abi-linux" = { path = "modules/abi/linux", enabled = true }
"community-net-stack" = { git = "https://github.com/org/net", rev = "abc123", enabled = false }
```

This keeps the file readable, keeps local / `crates.io` / git sources visibly distinct, and still preserves the `.config` property that disabled options remain explicitly present.

### Module entry fields

Each module entry must contain:

- `enabled = true | false`
- exactly one normal Cargo-like source form:
  - `version = "..."` for the default registry
  - `version = "...", registry = "..."` for a non-default registry
  - `path = "..."`
  - `git = "..."` with an accompanying selector such as `rev`, `branch`, or `tag`

Optional fields may follow Cargo dependency conventions when needed, such as `package = "..."` for renamed packages or `features = [...]` / `default-features = false`.

#### `enabled`

`enabled` records the resolved on/off state for that module option.

If `enabled = true`, the option is eligible for emission into the generated `.scarlet/scarlet-modules` crate.

If `enabled = false`, the option stays visible in the config but is not emitted into the generated dependency graph.

This explicit false state is central to the `.config`-style model.

Explicit `false` should be treated as authoritative.

That means if an enabled module option depends on another option that is explicitly `enabled = false`, the build tool must report a configuration error instead of silently enabling the dependency.

### MVP scope

For the prototype and MVP, module entries are written directly in `scarlet-config.toml`.

That means the file is allowed to carry the dependency source information needed to synthesize `.scarlet/scarlet-modules`.

More advanced metadata systems can be added later if needed, but they are not required for the current model.

## Generated Output Mapping

For each `enabled = true` module, the build tool writes the corresponding dependency into the project-local generated crate.

Conceptually:

```text
scarlet-config.toml
  -> filter enabled modules
  -> read dependency-like module entries
  -> convert resolved sources into Cargo dependencies
  -> generate <project-root>/.scarlet/scarlet-modules/Cargo.toml
  -> generate <project-root>/.scarlet/scarlet-modules/src/lib.rs
```

Example generated dependency mapping:

```toml
[dependencies]
scarlet-driver-pl011 = { version = "0.1.0" }
scarlet-abi-linux = { path = "../../../modules/abi/linux" }
```

## Image Template Expansion

Image steps may use template placeholders in command paths, arguments, working
directories, environment values, input sources, input destinations, and output
paths.

Supported placeholders:

- `{project}` — current Scarlet project root
- `{repo}` — current working directory used to invoke `cargo-scarlet`
- `{kernel_elf}` — kernel ELF path selected for the image command
- `{profile}` — `debug` or `release`
- `{target_triple}` — resolved Rust target triple
- `{board}` — board name from `[board]`
Project-specific artifact locations should be declared directly in
`scarlet-config.toml`. For example:

```toml
args = [
    "{repo}/mkfs/rootfs",
    "{repo}/user/bin/dist/aarch64",
    "{repo}/mkfs/dist/modules/{target_triple}",
]
```

This keeps the artifact contract in the legacy project-local build adapter. New
distro/image composition should declare installable files in layer recipes and
let `cargo-scarlet plan` generate the image plan.

## Validation Rules

The build tool should validate at least the following:

1. `config_version` is supported
2. project root is writable for `.scarlet/scarlet-modules` generation
3. `board.target_json` exists
4. `kernel` has a usable source (`version`, `git`, or `path`)
5. every module entry contains an explicit `enabled` field
6. every module entry uses exactly one valid Cargo-like source form
7. every enabled module resolves to a valid Cargo dependency
8. the generated dependency graph must successfully resolve under Cargo metadata inspection

### Validation layers

Validation should happen in two layers:

1. **Config validation**
   - check basic schema validity
   - check `enabled` presence
   - check valid source forms
2. **Cargo graph validation**
   - run `cargo metadata` against the project after `.scarlet/scarlet-modules` is generated
   - inspect the resolved package and feature graph
   - fail if Cargo resolution exposes dependency or feature breakage

Cargo's resolved graph is the final truth for actual dependency and feature interactions.

### Notes on contradictions

For the MVP, contradictions and dependency breakage should primarily be surfaced by Cargo's own resolver through `cargo metadata`.

That keeps the prototype simple while still validating the real resolved graph.

## Minimal Example

```toml
config_version = 1

[project]
name = "qemu-riscv64-dev"

[board]
name = "riscv64-limine-full"
target = "riscv64gc-unknown-none-elf"
target_json = "kernel/targets/riscv64gc-unknown-none-elf.json"

[kernel]
package = "scarlet"
source = { version = "0.16.0" }

[kernel.features]
network = true
user-fpu = true
user-vector = true
hypervisor = false
limine = true
profiler = false

[modules]
"scarlet-driver-pl011" = { version = "0.1.0", enabled = true }
"scarlet-abi-linux" = { version = "0.1.0", enabled = true }
"scarlet-driver-ns16550" = { version = "0.2.0", enabled = false }
```

For the in-tree Scarlet development repository, the same resolved option set may coexist with local-path registry entries during development.

## Non-Goals

This format does not currently attempt to describe:

- runtime-loadable modules
- per-module arbitrary build scripts in config
- complete Cargo profile settings
- user-space package composition

Those can be layered on later if needed, but they should not blur the core responsibility of this file: build-time kernel/module composition.

## Summary

`scarlet-config.toml` is the declarative contract between Scarlet's build tool and its generated project-local module aggregation crate.

In one sentence:

> the file says what the system should be built from, and the tool turns that into `.scarlet/scarlet-modules` for the chosen project.
