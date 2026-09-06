# Scarlet build system

Scarlet uses project-rooted `scarlet.toml` manifests and `cargo-scarlet`
from the separate [scarlet-sdk repository](https://github.com/petitstrawberry/scarlet-sdk).
The SDK supplies build tooling and image plugins, not the native user libraries
or the Scarlet Rust compiler.

This is the integration guide, reviewed on 2026-09-06. The SDK's own
[tooling contract](https://github.com/petitstrawberry/scarlet-sdk/blob/main/docs/1.0-contract.md)
defines its CLI, manifest, lock, and plugin boundary. The package versions,
manifest schema, Cargo locks, project locks, and kernel ABI are separate things.

## Quick start

From the Scarlet root in the Nix development shell:

```sh
# Kernel/BSP only.
cargo scarlet build --project projects/riscv64-limine-full

# Build kernel and compose the project's declared images.
cargo scarlet image --project projects/riscv64-limine-full

# Compose release images, then launch the project runner.
cargo scarlet run --project projects/riscv64-limine-full --release
```

`cargo make build-riscv64` / `build-aarch64` build kernel and core user
components; they do not perform the full `image` operation.
`cargo make run-riscv64` / `run-aarch64` wrap the release `run` commands.
See [userspace development](../userspace/README.md) for application-only builds.

### Cross C compiler selection

The Nix development shell and Docker image provide unwrapped Clang as
`TARGET_CC`, the cc-rs fallback for cross compilation. Native C builds keep
using `HOST_CC`/`CC` and the Nix compiler wrapper. Avoid exporting global
`CC_<target>` defaults: they override application settings in Cargo's `[env]`
table, including `yt-for-scarlet`'s cross GCC selection.

Applications can select a different cross compiler with `CC_<target>` in
their own `.cargo/config.toml`; cc-rs checks that before `TARGET_CC`. This
controls C headers and compilation only, not Rust `std` support. The existing
`cargo make test-cross-cc` task checks Scarlet targets, application overrides,
and native compiler selection in the Nix shell. After updating the flake,
reload direnv or re-enter the shell to discard old environment variables.

## Project layout

The tracked reference projects are
[RISC-V full](../../projects/riscv64-limine-full/scarlet.toml),
[AArch64 full](../../projects/aarch64-limine-full/scarlet.toml), and
[AArch64 microvm](../../projects/aarch64-limine-microvm/scarlet.toml).
Their layout is:

```text
projects/<project>/
  scarlet.toml                  # Project, BSP, modules, images, runner
  scarlet.lock                  # Resolved image/layer inputs
  bsp/
    Cargo.toml, Cargo.lock      # Executable BSP and its Rust dependencies
    build.rs, src/main.rs       # Link configuration and boot entry
    lds/                        # BSP linker scripts
    .cargo/config.toml          # Bare-metal kernel target/build-std
  tools/                        # Project runner and helpers
  .scarlet/
    scarlet-modules/            # Generated kernel/module aggregation crate
    images/                    # Composed images and stamps
    staging/                   # Intermediate filesystem trees
    cache/                     # Git, downloads, child Cargo state and outputs
```

There are no separate tracked desktop or Apple Limine projects in this tree.
External BSP repositories and historical board investigations have their own
scope. Do not use their paths as checked-in reference project names.

## CLI

| Command | Scope |
| --- | --- |
| `build --project <path>` | Generate module aggregation, build the BSP, attempt kernel-symbol sidecar injection |
| `check --project <path>` | Type-check the BSP and its dependencies, not all image layers |
| `clippy --project <path>` | Run Clippy for the BSP; trailing `-- ...` supplies child arguments |
| `image --project <path>` | Build the kernel and compose declared images |
| `run --project <path>` | Compose images and invoke `[runner].command`; trailing arguments go to the runner |
| `update --project <path>` | Refresh BSP Cargo dependencies and resolve layer sources into `scarlet.lock` |
| `new --project <name>` | Scaffold an editable project; not a ready-made board port |
| `new --lsm <name>` | Scaffold a loadable Scarlet module |
| `build --lsm <path>` | Build a loadable module, instead of a project |

Project commands require `--project`; `build` does not default to the current
directory. The LSM alternatives use `--lsm`, not `--module`.

| Option | Applies to | Meaning |
| --- | --- | --- |
| `--release` | build, check, clippy, image, run | Select release profile |
| `--target <target>` | build, check, clippy, image, run | Override the build target; LSM builds require a target JSON path |
| `--locked` | project build, image, run | SDK project-lock behavior described below, not all Cargo flags |
| `--no-build` | image | Skip kernel build; Cargo/script image layers can still execute |
| `--kernel-elf <path>` | image | Select the kernel input; use with `--no-build` for a prebuilt kernel |
| `--no-image` | run | Skip image composition and invoke the runner |
| `--output <dir>` | LSM build | Copy the built module into this directory |

`cargo scarlet --offline` has been removed. The SDK does not implement a
network sandbox or forward every Cargo option to arbitrary child processes.

## Project manifest (schema 2)

Current reference projects use `[bsp]`. For example, the BSP/module portion
of a project under `projects/<name>/` is:

```toml
schema_version = 2

[project]
name = "my-scarlet-project"

[bsp]
path = "bsp"
package = "scarlet"

[bsp.kernel]
source = { path = "../../kernel" }
features = { network = true, user-fpu = true, user-vector = true, limine = true, hypervisor = false }

[modules]
"scarlet-module-prototype" = { path = "../../modules/scarlet-module-prototype", enabled = true }
```

`[bsp].path` is relative to the project. The BSP's `.cargo/config.toml`
must define `[build].target`; the reference BSPs point to JSON files in
`kernel/targets/`. Kernel source paths remain relative to the project
manifest, not to `bsp/`. Kernel features can be a list of enabled names or
a table of boolean states. Explicit false states are checked against Cargo
feature unification.

The older `[kernel]` form remains accepted, with `package`, `source`,
`target`, `target_json`, and a boolean `features` table. It uses the project
root as the executable BSP root. It is distinct from the reference projects'
current `bsp/` layout.

Kernel sources accept paths or Git tables, for example:

```toml
[bsp.kernel]
source = { git = "https://github.com/petitstrawberry/Scarlet", branch = "dev" }
features = ["network", "limine"]
```

Kernel and static-module Git dependencies are rendered as Cargo dependencies
in `scarlet-modules`; the BSP's `Cargo.lock` resolves them. Do not confuse
that with SDK-managed Git image-layer checkouts and `scarlet.lock`.

### Static modules versus LSMs

Enabled `[modules]` entries are ordinary Rust library crates linked through
the generated `scarlet-modules` crate. Each exposes `force_link()`; the BSP
calls `scarlet_modules::force_link()` to retain module initialization.

Entries accept `path`, Cargo-like `git` with `rev`/`branch`/`tag`, or
`version`/`registry`, plus `features`, `default-features`, and `enabled`.
Disabled entries are omitted from aggregation. These are not `.lsm` files
and do not require an LSM scaffold. See the
[static prototype](../../modules/scarlet-module-prototype/src/lib.rs) and
[loadable module guide](../modules/lsm.md).

### Local overrides

`scarlet.local.toml` merges over `scarlet.toml` before typed parsing:

```toml
[bsp.kernel.features]
hypervisor = true
```

Tables merge recursively; arrays append; other values are replaced by the local
value, including a change of representation. This is not array deletion or a
second lock file. Keep local overrides outside version control.

## Images and ordered layers

Images use `[images.<name>]` with `format`, `output`, optional `deps`,
and ordered `layers`. A consuming image must declare its dependencies;
cycles and unknown dependency names are errors.

| Format | Operation |
| --- | --- |
| `newc` | Compose a CPIO newc filesystem archive |
| `ext2` | Compose an ext2 filesystem image |
| `gpt-ext2` | Compose a single-ext2-partition GPT image |
| `gpt` | Combine declared partition payload images |
| `limine-uefi` | Invoke the matching Limine image plugin |

Full boot/rootfs recipes are in the reference manifests, not an implicit global
SDK image. In particular, AArch64 full composes a FAT ESP and ext2 rootfs into
one GPT disk; RISC-V full uses separate boot/rootfs images. See
[Limine boot](../boot/limine.md).

Layers are applied in declaration order. Local paths are relative to the file
declaring the layer. Path templates include `{arch}`, `{target_triple}`, and
`{project}`. Later filesystem layers may replace earlier files.

### Bundle

A local bundle expands its `[[layers]]` at the point of declaration:

```toml
[[images.rootfs.layers]]
kind = "bundle"
path = "../../bundles/full/bundle.toml"
```

Git bundles use `source = { git = "...", rev = "<commit>" }`, optional
`subdir`, and `bundle` (default `bundle.toml`). Nested bundle paths are
relative to each declaring bundle. Pin a Git bundle revision when selecting
release inputs; the expanded package lock does not itself lock the bundle.

### Copy and archive

```toml
[[images.rootfs.layers]]
kind = "copy"
source = "rootfs"
to = "/"
```

Copy sources can be local files/directories or URLs. Downloaded files are cached
and checked against recorded hashes where available. `template = true` enables
the existing copy-template processing.

An `archive` layer requires `source`, `to`, `format` (`tar`, `tar-gz`,
`tar-zst`, or `tar-xz`), and `sha256`, with optional `strip_components`.
The hash can be a single value or an architecture-keyed table. Checksums are
verified before extraction; unsupported/missing architecture hashes fail.
The SDK tooling contract records the supported extraction inputs and limits.

### Cargo

A bundle can build and install a normal std application:

```toml
[[layers]]
kind = "cargo"
source = "../../user/std-bin"
package = "scarlet-std-bin"
bin = "hello"
to = "/system/scarlet/bin/hello"
```

For external applications, `source` can be a Git table and `subdir` selects
a package below the checkout root. For example, ScarletUI's widget factory uses
`subdir = "examples/widget-factory"`, `package = "scarlet-ui-widget-factory"`,
and `bin = "scarlet-ui-widget-factory"`.

`features` and `default-features` control this layer's Cargo build.
`replace = true` removes earlier **Cargo** layers with the same `to`;
it does not remove unrelated layer kinds. Keep optional-application feature
recipes with their bundles, such as
[video-player](../../user/video_player/README.md).

### Script

A script layer invokes `sh <source> <output-path>` from the project directory,
then installs the output at `to`. `output` may name a reusable output path;
its recorded hash does not track every external input a script could read.
Use trusted scripts only: they execute with host access.

### Image

```toml
[images.boot]
format = "limine-uefi"
output = ".scarlet/images/boot.img"
deps = ["initramfs"]

[[images.boot.layers]]
kind = "image"
source = "initramfs"
to = "/boot/initramfs"
```

This references another declared image's output; the example assumes an
`images.initramfs` section. GPT partitions similarly select named image
payloads with explicit `deps`; see the AArch64 full manifest.

## Locks, caches, and network access

`scarlet.lock` records resolved layer identities, package Git revisions, and
input/output hashes under image sections. `update` and image composition write
it; commit the selected distributable lock at release staging.

- Matching Git Cargo layer entries reuse their recorded revisions. Missing or
  changed entries can be resolved from the manifest.
- Local source edits remain development inputs; a project lock does not hash
  and freeze an entire local Cargo source tree.
- A BSP `Cargo.lock` and each userspace Cargo workspace lock separately govern
  transitive Rust dependencies.
- Incremental image stamps/cache hits are not proof that arbitrary script or
  host-tool inputs are reproducible.

`--locked` retains the SDK's project-lock semantics: archive-layer inputs
must match the existing lock, and the explicit BSP `cargo update` before
build is skipped. It does **not** reject every possible manifest change,
make image composition read-only, or propagate Cargo's `--locked` everywhere.
Missing/mismatched Git package entries may still resolve; composition still
saves its resulting lock.

Build, check, image, and run can cause network access through Git bundle/layer
resolution, Cargo, scripts, plugins, or runners. `update` is not the only
network-using command. The former partial `--offline` restriction has been
removed; caches and `--locked` are not network isolation.

SDK-managed caches are project-local: `.scarlet/cache/git`, `files`,
`target` (isolated by package source root), and `cargo-home`. Child Cargo uses
that project-local `CARGO_HOME`, not the caller's normal Cargo cache. Retaining
caches does not require an `--offline` mode.

## Plugin and execution hooks

`limine-uefi` invokes `cargo-scarlet-plugin-limine` through `PATH`.
Other unknown formats are errors; the implementation does not discover
arbitrary plugins by a format-name prefix. Install the CLI and plugin from a
matching SDK revision, using the SDK README instructions.

The CLI writes one JSON request to plugin stdin with `project_dir`,
`section_name`, `format`, `arch`, `kernel_elf`, `initramfs`, `output`,
and `section`. The section supplies `cmdline`, optional `dtb`, and resolved
`packages` containing local source and absolute FAT destination paths.
The plugin reports success/failure by exit status; there is no structured
stdout result or protocol-version negotiation.

`[hooks.post-image]` accepts `command` and optional `args`. It runs after
successful image composition and lock saving, from the project directory,
with `SCARLET_PROJECT_DIR`, `SCARLET_TARGET_TRIPLE`, and `SCARLET_PROFILE`
(`debug` or `release`). A failing hook fails the operation; it does not
roll back already written images or locks.

`[runner].command` also runs from the project directory. Release runs set
`SCARLET_RELEASE=1`, and trailing CLI arguments go to the runner. The runner
owns emulator/device configuration. Neither hooks nor runners are sandboxed.

## Generated files and scaffolding

Aggregation `Cargo.toml` / `src/lib.rs`, staging trees, and images under
`.scarlet/` are SDK-owned artifacts. Do not manually patch generated
dependencies; change source manifests instead. Aggregation
`.cargo/config.toml` is initialized only when missing, so existing
project-specific configuration is retained.

```sh
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf --kernel-path kernel
cargo scarlet new --lsm my-module --kernel-path kernel
```

Run these from a checkout with the indicated kernel path, using new destination
names. The current project scaffold uses the legacy project-root BSP layout:
`Cargo.toml`, `build.rs`, `src/main.rs`, `lds/`, `.cargo/config.toml`,
`scarlet.toml`, and `.scarlet/scarlet-modules/`. It does not clone a reference
board or create the modern reference projects' `bsp/` subdirectory.

After scaffolding, configure actual target JSON paths (including the `.json`
suffix), linker scripts, build-std, boot entry, images, and runner. The emitted
boot entry is a placeholder. For a current Limine project, use the tracked
reference BSPs as the implementation guide. LSM scaffolding instead emits
`module.toml`, `src/lib.rs`, and its Cargo/build configuration.

## See also

- [Kernel development](../kernel/README.md)
- [Userspace development](../userspace/README.md)
- [Distribution model](../architecture/distro-model.md)
- [LSM](../modules/lsm.md)
