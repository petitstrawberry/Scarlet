# Scarlet Distribution Model

This is the current plan for issue #461. It supersedes the earlier
BSP/Image/Distro split proposal.

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
- `scarlet.lock` is per project and pins resolved git revisions and content
  hashes for reproducible image composition.
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

Future split repositories
  scarlet-kernel, scarlet-coreutils, scarlet-ui, apps, drivers, and modules can
  become independent sources once their boundaries are stable.
```

This keeps the convenient clone-and-run workflow while allowing the same
manifest format to consume path, git, URL, and generated sources.

## Terms

| Term | Meaning |
|---|---|
| Project | A directory with `scarlet.toml`, project Cargo files, boot entry, linker scripts, runner, generated `.scarlet/`, and optional project-local files. |
| Project variant | A named project such as `aarch64-limine-full`, `aarch64-limine-desktop`, or `aarch64-limine-microvm`. |
| BSP boundary | The project directory itself: target, boot entry, linker scripts, runner, and board/virtualization assumptions. |
| Distro shape | The selected ordered layer composition for initramfs/rootfs/boot in a project manifest. |
| Layer | One ordered composition operation such as `bundle`, `cargo`, `copy`, `script`, or `image`. |
| Bundle | A reusable TOML file containing ordered `[[layers]]`. |
| Boot image format | The boot packaging boundary, for example `limine-uefi`. Formats are handled by plugins. |
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
      tools/
      .scarlet/
    riscv64-limine-desktop/
    aarch64-limine-full/
    aarch64-limine-desktop/
    aarch64-limine-microvm/
    aarch64-apple-limine-full/
  docs/
  flake.nix
```

There is intentionally no in-repository `cargo-scarlet/` implementation
directory in the current layout. The CLI is supplied by `scarlet-sdk`.

## Manifest Schema

Every active project manifest uses schema version 2:

```toml
schema_version = 2

[project]
name = "scarlet-aarch64-limine-microvm"

[kernel]
package = "scarlet"
source = "../../kernel"
target = "aarch64-unknown-none-elf"
target_json = "../../kernel/targets/aarch64-unknown-none-elf.json"

[kernel.features]
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
to = "/system/scarlet/bin/init"

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
source = "fs"
to = "/"

[[layers]]
kind = "cargo"
source = "../../user/bin"
package = "user-bin"
bin = "sh"
to = "/system/scarlet/bin/sh"
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

The active lock format is section-based:

```toml
[sections.rootfs]
hash = "sha256:..."

[[sections.rootfs.layers]]
kind = "cargo"
source = "../../user/bin"
package = "user-bin"
bin = "sh"
to = "/system/scarlet/bin/sh"
hash = "sha256:..."
```

External git sources record `resolved_rev`.

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
cargo scarlet new --module my-module
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
