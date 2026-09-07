# Scarlet Distribution Model

This records the project-centric direction from issue #461, reconciled with
the current layout on 2026-09-06. It supersedes the earlier BSP/Image/Distro
split proposal. For executable CLI/layer details, use the
[build-system guide](../build-system/README.md).

Scarlet is treated as the official reference distribution of the Scarlet OS
platform. This repository keeps the known-good composition of kernel, modules,
bundles, userland, image recipes, boot configuration, and run workflows. The
build and image tooling lives in `scarlet-sdk` and is consumed here through
`cargo scarlet`.

## Current Direction

The model is project-centric:

- A project is the buildable Scarlet target and local BSP boundary.
- `scarlet.toml` is the project manifest and the single source of truth.
- `schema_version = 2` is the active manifest schema.
- Images are composed from ordered layers.
- Bundles are reusable ordered layer collections.
- `scarlet.lock` is per project and records resolved layer revisions and content
  hashes; Cargo dependency locks and external script inputs remain separate.
- `scarlet.local.toml` is a gitignored per-developer override file.

The earlier `scarlet-config.toml`, `bsp.toml`, `kernel.toml`, standalone
`image.toml`, and Yocto-style `machines/`, `distros/`, `recipes/`, and
`packagegroups/` directories are no longer the target model.

## Repository Roles

```text
petitstrawberry/Scarlet
  Official reference distribution and integration repository.
  Contains kernel source today, in-tree userland, bundles, projects, and locks.

petitstrawberry/scarlet-sdk
  Build, check, clippy, image, update, scaffold, and run tooling.
  Provides cargo-scarlet and image plugins through the development environment.

petitstrawberry/scarlet-ui and petitstrawberry/sgfx
  Already separate UI/graphics sources, consumed through Cargo and bundle layers.

Possible future splits
  Kernel, core utilities, other libraries, apps, drivers, and modules can
  become independent sources where their boundaries justify it.
```

This keeps the convenient clone-and-run workflow while allowing the same
manifest format to consume path, git, URL, and generated sources.

## Terms

| Term | Meaning |
|---|---|
| Project | A directory with `scarlet.toml`, a selected BSP, image/runner configuration, generated `.scarlet/`, and optional project-local files. |
| Project variant | A named project such as `riscv64-limine-full`, `aarch64-limine-full`, or `aarch64-limine-microvm`. |
| BSP boundary | The executable Cargo package selected by `[bsp].path` (currently `bsp/`), with target, boot entry and linker scripts; image and runner policy remains in the project. |
| Distro shape | The selected ordered layer composition for initramfs/rootfs/boot in a project manifest. |
| Layer | One ordered composition operation: `bundle`, `cargo`, `copy`, `archive`, `script`, or `image`. |
| Bundle | A reusable TOML file containing ordered `[[layers]]`. |
| Boot image format | The packaging boundary: built-in filesystem/GPT formats or `limine-uefi` through the matching Limine plugin. |
| Lock file | Project-local `scarlet.lock` with section hashes, layer hashes, and resolved git revisions. |

`microvm` is not a QEMU machine name in this model. It is a project/distro
variant that happens to run on QEMU `virt` with virtualization enabled by the
project runner.

`limine-uefi` is not a distro by itself. It is a boot image format and plugin
boundary. A project may still be named `*-limine-*` when Limine is part of the
observable target identity.

## Current Layout

```text
Scarlet/
  kernel/
  user/
    bin/
    lib/
    std-bin/
  modules/
  drivers/
  bundles/
    base/
      bundle.toml
      fs/
    cli-utils/
      bundle.toml
    desktop/
      bundle.toml
      fs/
      tools/
    experimental/
      bundle.toml
    linux/
      bundle.toml
      rootfs/
    full/
      bundle.toml
  projects/
    riscv64-limine-full/
      scarlet.toml
      scarlet.lock
      bsp/
        Cargo.toml
        Cargo.lock
        .cargo/config.toml
        build.rs
        src/main.rs
        lds/
      tools/
      .scarlet/
    aarch64-limine-full/
    aarch64-limine-microvm/
  docs/
  flake.nix
```

There is intentionally no in-repository `cargo-scarlet/` implementation
directory in the current layout. The CLI is supplied by `scarlet-sdk`.

## Manifest Schema

Every active project manifest uses schema version 2. This microvm excerpt shows
the BSP and boot composition; see the
[complete manifest](../../projects/aarch64-limine-microvm/scarlet.toml) for all images:

```toml
schema_version = 2

[project]
name = "scarlet-aarch64-limine-microvm"

[bsp]
path = "bsp"
package = "scarlet"

[bsp.kernel]
source = { path = "../../kernel" }

[bsp.kernel.features]
network = true
user-fpu = true
user-vector = true
hypervisor = true
limine = true
profiler = false

[modules]
"scarlet-module-prototype" = { path = "../../modules/scarlet-module-prototype", enabled = false }

[images]

[images.initramfs]
format = "newc"
output = ".scarlet/images/initramfs-aarch64-microvm.cpio"

[[images.initramfs.layers]]
kind = "bundle"
path = "../../bundles/base/bundle.toml"

[[images.initramfs.layers]]
kind = "cargo"
source = "../../user/bin"
package = "user-bin"
bin = "microvm-init"
to = "/init"

[images.boot]
format = "limine-uefi"
output = ".scarlet/images/limine-aarch64-microvm.img"
cmdline = "console=ttyAMA0"
deps = ["initramfs"]

[[images.boot.layers]]
kind = "image"
source = "initramfs"
to = "/boot/initramfs"

[runner]
command = "tools/run_aarch64.sh"
```

## Layers

Layers are applied in declaration order. Later layers can overwrite files from
earlier layers.

Supported layer kinds:

| Kind | Purpose |
|---|---|
| `bundle` | Expand another `bundle.toml` at this exact position. |
| `cargo` | Build a Cargo binary and install it into the image. |
| `copy` | Copy a file or directory into the image. |
| `archive` | Verify a SHA-256-pinned archive and extract it into the image. |
| `script` | Run a script and install its declared output. |
| `image` | Include another image output, such as initramfs in a boot image. |

All relative paths are resolved from the TOML file where they appear. This lets
project manifests and bundles remain relocatable.

## Bundles

Bundles are the scalable replacement for hardcoded image package groups. They
contain ordered layers, not packages:

```toml
# bundles/base/bundle.toml
[[layers]]
kind = "copy"
source = "fs/systems"
to = "/systems"

[[layers]]
kind = "cargo"
source = "../../user/bin"
package = "user-bin"
bin = "sh"
to = "/systems/scarlet/bin/sh"
```

Projects reference bundles through image layers:

```toml
[[images.rootfs.layers]]
kind = "bundle"
path = "../../bundles/full/bundle.toml"
```

The current reference distro uses these bundle roles:

| Bundle | Role |
|---|---|
| `base` | Minimal system files and core Scarlet services/commands. |
| `cli-utils` | Common command-line utilities. |
| `desktop` | Desktop shell, UI demos, media, input, and desktop support files. |
| `linux` | Architecture-specific Linux guest artifacts and config files. |
| `experimental` | External or experimental apps, including git sources. |
| `full` | Composes desktop, Linux, and experimental layers. |

## Locking

`cargo scarlet update --project <project>` resolves network sources and writes
`scarlet.lock`.

`cargo scarlet image` and `cargo scarlet run` use the project lock while
composing images. Lock files are committed per project so each project can pin
the exact source revisions and content hashes that belong to that image shape.
Composition can resolve missing/changed inputs and save the resulting lock.
`--locked` is the SDK's archive-input check and BSP-refresh control, not a
blanket Cargo flag or network restriction. `--offline` has been removed; see
[lock/cache scope](../build-system/README.md#locks-caches-and-network-access).

The active lock format is section-based. This illustrative excerpt uses
placeholder hashes, not a usable lock:

```toml
[sections.rootfs]
hash = "sha256:..."

[[sections.rootfs.layers]]
kind = "cargo"
package = "user-bin"
bin = "sh"
to = "/systems/scarlet/bin/sh"
hash = "sha256:..."

[sections.rootfs.layers.source]
type = "path"
path = "../../user/bin"
```

External git sources record `resolved_rev`.
Git bundle definitions need their own pinned selector, and the BSP/userspace
Cargo locks separately select transitive Rust dependencies.

## CLI

Current commands are:

```sh
cargo scarlet build --project projects/riscv64-limine-full
cargo scarlet check --project projects/riscv64-limine-full
cargo scarlet clippy --project projects/riscv64-limine-full
cargo scarlet image --project projects/riscv64-limine-full
cargo scarlet run --project projects/riscv64-limine-full --release
cargo scarlet update --project projects/riscv64-limine-full
cargo scarlet new --project my-board --target riscv64gc-unknown-none-elf
cargo scarlet new --lsm my-module
```

The standalone `scarlet` CLI can still be added later, but the current
implementation path is to keep `cargo scarlet` as the operational frontend and
share core logic from `scarlet-sdk`.

## Build And Run Flow

```text
cargo scarlet run --project projects/aarch64-limine-microvm --release
  -> read scarlet.toml and scarlet.local.toml
  -> resolve kernel, modules, image sections, and ordered layers
  -> generate .scarlet/scarlet-modules/
  -> build the project kernel
  -> compose initramfs/rootfs/boot images
  -> invoke the project runner from [runner]
```

For Limine projects, the boot image is built by the Limine plugin. The runner
then launches QEMU with the project-specific device and machine flags.

## Migration Status

Completed or effectively in place:

- [x] `Scarlet` acts as the official reference distro/integration repository.
- [x] `scarlet-sdk` supplies `cargo-scarlet` through the development
  environment.
- [x] Project manifests use `schema_version = 2`.
- [x] `scarlet-config.toml` has been removed from active projects.
- [x] Image composition is manifest-driven via ordered layers.
- [x] Reusable bundles exist under `bundles/`.
- [x] In-tree apps can be included or excluded by image layer selection.
- [x] External git app sources can be included by layers.
- [x] Per-project `scarlet.lock` files pin resolved sources and image hashes.
- [x] AArch64 microvm has a project manifest and `cargo make run-aarch64-microvm`
  dispatches to `cargo scarlet run --project projects/aarch64-limine-microvm`.

Remaining work:

- [x] Update the issue body and architecture document so they supersede the old
  BSP/Image/Distro proposal preserved in historical comments.
- [ ] Decide how much of `user/bin` should remain a multi-binary package versus
  moving stable utilities into split packages/repositories.
- [ ] Move kernel, core utilities, libraries, drivers, modules, and apps to
  external repositories only when their interfaces are stable enough.
- [ ] Keep `scarlet.local.toml` consistently ignored and documented.
- [x] Remove stale scripts or docs that assume an in-tree `cargo-scarlet/`
  implementation directory.
- [ ] Add a standalone `scarlet` CLI only if platform/workspace operations grow
  beyond what fits naturally in `cargo scarlet`.

## Non-Goals

- Do not reintroduce Yocto-style metadata directories as the primary model.
- Do not make `microvm` a machine abstraction just because QEMU is used.
- Do not make Limine a distro abstraction; it is a boot image format boundary.
- Do not require a physical repository split before project manifests and
  bundles are stable.
- Do not use git submodules as the primary composition mechanism.
