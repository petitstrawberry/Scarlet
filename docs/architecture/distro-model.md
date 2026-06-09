# Scarlet Distro Architecture

This document replaces the early `project` / `scarlet-config.toml` model with a
scalable distribution model for Scarlet before the physical repository split.

The goal is not to copy Yocto or Zephyr. The goal is to keep the useful
separation they prove:

- workspace source revisions are not distribution policy
- machine support is not an image recipe
- a buildable program is not automatically installed into an image
- generated execution plans are not user-authored configuration

`petitstrawberry/Scarlet` is the official reference distribution of the Scarlet
OS platform. It remains the easiest "clone and run" repository, but its checked
in metadata must describe a known-good composition of kernel, tooling, libraries,
packages, apps, machines, boot targets, and images rather than implying that all
source trees in the repository are installed into every image.

## Problems in the Previous Model

The previous design still mixed several responsibilities:

- `Distro Manifest` meant both "where source repositories come from" and
  "what the distro policy is".
- `BSP` included kernel feature selection, even though those features can vary
  by distro, image, or debug profile without changing the board.
- `Image Recipe` pointed at a BSP directly, which makes images less reusable
  across machines.
- `package`, `component`, `app`, and `module` were used for source repos,
  build recipes, installable outputs, and kernel loadable objects.
- `image.steps` was treated as user-facing config even though it is really a
  low-level execution plan.

Those ambiguities make the split-repo direction hard to implement. The model
below gives every file one job.

## Terms

Use these terms consistently.

| Term | Meaning |
|---|---|
| Workspace | A checked-out set of source repositories used by one build. |
| Workspace manifest | `scarlet.toml`; declares source repos/layers and desired revisions. |
| Lock file | `scarlet.lock`; pins the exact resolved revisions. |
| Layer | A reusable metadata unit containing machines, boot targets, distros, recipes, package groups, and images. |
| Machine | Hardware/virtual hardware contract. No boot image policy and no userland package list. |
| Boot target | Boot packaging contract such as Limine UEFI, direct kernel boot, m1n1+U-Boot, or a signed device image. |
| Distro | OS policy: default providers, feature policy, filesystem policy, ABI policy, release identity, and default boot target. |
| Recipe | Build instructions for one source unit. A recipe can produce one or more packages. |
| Package | Installable output selected by images or package groups. |
| Package group | Named set of packages used to keep images small and readable. |
| Image recipe | Root filesystem composition: package groups, packages, users, files, services. |
| Build profile | Optimization/debug/signing/cache choices for a build invocation. |
| Image plan | Generated `.scarlet/plans/.../image-plan.toml`; executable steps resolved from the above. |
| Kernel module | Runtime-loadable kernel object. Do not call it a package or app. |
| Source | A path or git checkout used by a layer or recipe. Source provenance is not package selection. |

## Layers

Scarlet should use layers as the unit that can later move to separate
repositories. A layer can contain any of these directories:

```text
layers/<layer-name>/
  layer.toml
  machines/
  boot_targets/
  distros/
  recipes/
  packagegroups/
  images/
```

Layer order is explicit in `scarlet.toml`. Higher-priority layers may override
metadata from lower-priority layers by name, but overrides are whole-object
overrides in the first implementation. Fine-grained append files can be added
later if the need is real.

## Top-Level Files

### `scarlet.toml`: Workspace Manifest

`scarlet.toml` answers only "which metadata/source layers are present in this
workspace, and where do they come from?" It does not select a machine, boot
target, distro, or image.

```toml
schema_version = 1

[workspace]
name = "scarlet-reference"

[[layers]]
name = "scarlet-core"
path = "layers/scarlet-core"

[[layers]]
name = "scarlet-reference"
path = "layers/scarlet-reference"
priority = 100
```

Future split-repo form:

```toml
[[layers]]
name = "scarlet-core"
git = "https://github.com/petitstrawberry/scarlet-core"
rev = "..."
path = "layers/scarlet-core"
```

### `scarlet.lock`: Workspace Lock

`scarlet.lock` pins exact source revisions. It is generated from
`scarlet.toml`, local overrides, and fetch results. It does not contain image
package selection.

### `scarlet.local.toml`: Local Overrides

`scarlet.local.toml` is gitignored and only changes the developer workspace:

```toml
[[overrides.layers]]
name = "scarlet-core"
path = "../scarlet-core"
```

## Source Rules

Layer and recipe sources use the same provenance model:

- `path` points at a local checkout or in-tree source directory.
- `git` records the canonical remote.
- git sources must be pinned by `rev`, `branch`, or `tag`.
- `scarlet.lock` records the effective resolved revision for local git
  checkouts when available.
- `scarlet.local.toml` may replace or add layer sources for local development
  without changing the checked-in workspace manifest.

The first pre-split implementation resolves local paths and records git
provenance. Network fetch/update is a later SDK operation; the split boundary is
already represented by source metadata and lock output.

## Machine

Machine metadata describes the target hardware or virtual hardware. It must not
list userland packages and must not define a boot image. The same machine can
boot through different boot targets when the hardware supports it.

```toml
schema_version = 1

[machine]
name = "qemu-aarch64-virt"
arch = "aarch64"
target_triple = "aarch64-unknown-none-elf"
target_json = "../../../kernel/targets/aarch64-unknown-none-elf.json"

[machine.features]
serial = true
virtio_mmio = true
virtio_blk = true
virtio_gpu = false

[build_adapter]
project = "../../../projects/aarch64-limine-microvm"
```

`build_adapter` is a pre-split migration bridge. It lets the new resolver point
at the existing in-tree kernel binary project without making that project shape
part of the final architecture.

## Boot Target

Boot targets describe how the already-built kernel and initramfs become a
bootable artifact. Limine is a boot target because it abstracts a family of
loader contracts, but the selected boot target is still distinct from the
virtual or physical machine.

```toml
schema_version = 1

[boot_target]
name = "limine-uefi-aarch64"
kind = "limine-uefi"
arch = "aarch64"
output = "{project}/.scarlet/images/limine-aarch64-microvm.img"
cmdline = "console=ttyAMA0"
image_slack_mb = 8
limine_version = "11.0.0"
```

## Distro

Distro metadata describes policy. It does not choose a final image package
list and does not describe board wiring.

```toml
schema_version = 1

[distro]
name = "scarlet-microvm"
vendor = "petitstrawberry"
version = "0.17.0"
system_prefix = "/system/scarlet"
default_shell = "/system/scarlet/bin/sh"
default_boot_target = "limine-uefi-aarch64"

[providers]
kernel = "scarlet-kernel"
init = "scarlet-init"

[features]
network = true
desktop = true
hypervisor = true
```

## Recipe and Package

A recipe builds source. A package is an installable output from a recipe. The
image selects packages, not repositories and not recipes directly.

The first implementation can keep recipe metadata small:

```toml
schema_version = 1

[recipe]
name = "coreutils"
source = { path = "../../../user/bin" }
build = { kind = "cargo", package = "user-bin" }

[[package]]
name = "coreutils-base"
bins = ["cat", "ls", "cp", "mv", "rm", "mkdir", "echo", "uname"]
```

Later, when `user/bin` is split, each app can become its own recipe while
images continue to select packages by name.

Packages can also declare explicit installable files:

```toml
[[package]]
name = "shell"
files = [{ from = "dist/aarch64/sh", to = "/system/scarlet/bin/sh" }]
```

`from` is relative to the recipe source path. The generated image plan expands
these files into concrete inputs and writes an install manifest for rootfs
staging. If a recipe exists but its package is not selected by the image, its
files are not installed even if the source still builds as part of a larger
in-tree Cargo workspace.

## Image Recipe

Image recipes describe the root filesystem contents and image-specific
configuration. They are machine-neutral unless they explicitly declare machine
constraints.

```toml
schema_version = 1

[image]
name = "host"
description = "Host root filesystem for the Scarlet microVM distribution."
compatible_machines = ["qemu-aarch64-virt"]

packagegroups = ["boot", "core"]
packages = ["microvm-init"]

[initramfs]
format = "newc"
output = "{project}/.scarlet/images/initramfs-aarch64-microvm.cpio"

[rootfs]
format = "ext2"
output = "{project}/.scarlet/images/rootfs-aarch64-microvm.ext2"
source = "{workspace}/mkfs/rootfs"
user_bins = "{workspace}/user/bin/dist/aarch64"
modules = "{workspace}/mkfs/dist/modules/{target_triple}"
stage = "{project}/.scarlet/rootfs-stage"
prebuilt = "{project}/prebuilt"

[[files]]
source = "{workspace}/projects/aarch64-limine-microvm/initramfs"
destination = "/"

[[files]]
source = "{workspace}/user/bin/dist/aarch64/microvm_init"
destination = "/system/scarlet/bin/init"
```

The image recipe says what should exist in the root filesystem. It does not say
how to run `cpio`, `mkfs.ext2`, Limine, QEMU, or Cargo. Those are generated into
an image plan.

Image-level `[[files]]` entries are for image-specific static files and
directories. Installable app/package payloads should normally come from selected
recipe packages.

## Image Plan

The image plan is generated and tool-owned:

```text
.scarlet/plans/<machine>/<boot-target>/<distro>/<image>/<profile>/image-plan.toml
```

It is allowed to contain shell commands, expanded paths, build ordering,
artifact names, and executor-specific details. Users may inspect it, but normal
configuration should happen in machine/boot target/distro/image/recipe metadata.

The existing `[[image.steps]]` executor should be retained as an executor for
generated plans, not as the long-term user API.

For rootfs images, the generated plan writes an install manifest from selected
package files and passes it to the rootfs builder. This is the enforcement point
that separates "this app can be built" from "this app is installed in this
image".

## Build Selection

A complete build is selected by independent inputs. The boot target can be
explicit or selected from the distro default:

```bash
cargo scarlet plan \
  --machine qemu-aarch64-virt \
  --distro scarlet-microvm \
  --boot limine-uefi-aarch64 \
  --image host \
  --profile debug
```

Building the selected image uses the same tuple:

```bash
cargo scarlet image \
  --machine qemu-aarch64-virt \
  --distro scarlet-microvm \
  --image host
```

The tuple is:

```text
workspace manifest + machine + boot target + distro + image recipe + build profile
```

This avoids baking the machine into the image, the image into the machine, or
the boot packaging flow into either one.

## Pre-Split Repository Layout

Before splitting repositories, keep the code in this repository but introduce
the future metadata boundary:

```text
Scarlet/
  scarlet.toml
  scarlet.lock
  layers/
    scarlet-reference/
      layer.toml
      machines/
      boot_targets/
      distros/
      images/
      recipes/
      packagegroups/
  kernel/
  user/
  cargo-scarlet/
  projects/                 # temporary build adapters
```

`projects/` should shrink over time. It is not the final public model.

## Migration Order

1. Add `scarlet.toml`, layer metadata, and a resolver that generates an image
   plan for one known machine/boot/distro/image tuple.
2. Move user-authored image composition out of `scarlet-config.toml` and into
   `layers/*/images/*.toml`.
3. Make the existing `image.steps` runner consume generated
   `.scarlet/plans/.../image-plan.toml`.
4. Move kernel feature and module selection out of `scarlet-config.toml` into
   distro/profile/recipe metadata.
5. Split `user/bin` into recipes/packages so images can include or exclude
   programs without changing whether they build.
6. Add `scarlet.lock` generation and `scarlet.local.toml` overrides.
7. Split physical repositories by moving layers/source trees out one at a
   time. The reference `Scarlet` repo remains the official reference distro.

## Non-Goals

- Do not make `Scarlet` a kernel-only repository.
- Do not use git submodules as the primary composition mechanism.
- Do not require every app to be split before image composition becomes
  manifest-driven.
- Do not expose low-level image-plan commands as the normal user API.

## Acceptance Criteria Before Repository Split

- `scarlet.toml` exists and only describes workspace/layer sources.
- `scarlet.lock` can be generated and records effective layer revisions.
- `scarlet.local.toml` is gitignored and can override or add local layer
  sources.
- At least one layer contains a machine, boot target, distro, image recipe,
  package groups, and package recipes.
- `cargo-scarlet` can resolve a machine/boot/distro/image tuple into a
  generated image plan, with the boot target allowed to come from the distro
  default.
- `cargo-scarlet image --machine qemu-aarch64-virt --distro scarlet-microvm
  --image host` builds the microvm initramfs, rootfs, and Limine boot image from
  layer metadata.
- The generated plan records all selected metadata paths and expanded file
  inputs.
- In-tree apps can be represented as packages selected by image metadata,
  separate from whether `user/bin` builds, and unselected recipe files are not
  copied into the manifest-driven rootfs stage.
- Recipe sources support local `path` provenance and git provenance with an
  explicit pin (`rev`, `branch`, or `tag`).
- The old `project` model is treated as a temporary build adapter, not the
  architecture.
