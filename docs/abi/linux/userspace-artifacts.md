# Linux Userspace Artifacts

This document explains how to rebuild the Buildroot-generated root filesystem
and optional Linux user-space binaries (`zathura`, `green`, `fbdoom`, and
`kvmtool`) that power Scarlet's partial Linux ABI support.

## Host Requirements

Buildroot artifact generation is Linux-host work. Run these scripts in
`scarlet-dev`, a Linux VM, or a Linux Nix shell. macOS can still build Scarlet
itself, but the Buildroot host tools and generated toolchains are not supported
from macOS.

The helper scripts keep the Docker-compatible defaults, but every important path
can be injected:

- `BUILDROOT_DIR` - Buildroot checkout/build tree.
- `PREBUILT_DIR` - artifact staging directory.
- `WORKDIR` - checkout/build directory for optional user programs.
- `MAKE_JOBS` - parallelism for Buildroot.

## Building Buildroot

For RISC-V 64:

```bash
bash bundles/linux/tools/build_buildroot.sh
```

For AArch64:

```bash
ARCH=aarch64 bash bundles/linux/tools/build_buildroot.sh
```

Repository-local paths are useful outside the Docker image:

```bash
BUILDROOT_DIR="$PWD/.scarlet/cache/buildroot-aarch64" \
PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt" \
ARCH=aarch64 bash bundles/linux/tools/build_buildroot.sh
```

The resulting tarball is staged as
`$PREBUILT_DIR/{riscv64,aarch64}/rootfs.tar`.

The AArch64 Buildroot configuration enables the libraries needed by the Wayland
PDF viewer: Wayland client support, Cairo, GLib/GObject, GTK 3, and Poppler
GLib. The X keyboard data used by GTK/libxkbcommon is installed by
`build_user_programs.sh` and deployed as shared runtime data.

## Building Optional User Programs

With the Buildroot toolchain ready, build the auxiliary binaries:

```bash
ARCH=aarch64 \
BUILDROOT_DIR="$PWD/.scarlet/cache/buildroot-aarch64" \
PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt" \
WORKDIR="$PWD/.scarlet/cache" \
bash bundles/linux/tools/build_user_programs.sh
```

This script:

- Verifies the Buildroot cross toolchain and `pkg-config` wrapper.
- Clones or updates `green`, `fbdoom`, and `kvmtool`/`dtc` where applicable.
- Builds the zathura PDF viewer stack on AArch64: girara, zathura, and
  zathura-pdf-poppler.
- Installs binaries into `$PREBUILT_DIR/$ARCH/bin`, libraries into
  `$PREBUILT_DIR/$ARCH/lib`, and shared data into `$PREBUILT_DIR/$ARCH/share`.

`kvmtool` is only built for `riscv64`. It is skipped for `aarch64`.

## Building the KVM Guest Image

To create a bootable guest image for nested virtualization via Scarlet's
built-in hypervisor:

```bash
ARCH=aarch64 \
BUILDROOT_DIR="$PWD/.scarlet/cache/buildroot-aarch64" \
PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt" \
bash bundles/linux/tools/build_guest_image.sh
```

This cross-compiles a minimal Linux guest kernel, creates a compressed cpio
initramfs from the Buildroot rootfs, and stages `guest-Image` plus
`guest-initramfs.cpio.gz` under `$PREBUILT_DIR/$ARCH/bin`.

## Common Workflow

For AArch64 with repository-local artifacts:

```bash
export BUILDROOT_DIR="$PWD/.scarlet/cache/buildroot-aarch64"
export PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt"
export WORKDIR="$PWD/.scarlet/cache"

ARCH=aarch64 bash bundles/linux/tools/build_buildroot.sh
ARCH=aarch64 bash bundles/linux/tools/build_user_programs.sh
ARCH=aarch64 bash bundles/linux/tools/deploy_rootfs.sh
```

For RISC-V 64, use `buildroot-riscv64` as the local Buildroot directory and run
`ARCH=riscv64 bash bundles/linux/tools/build_buildroot.sh` or omit `ARCH`.

## Deployment

After building artifacts, deploy them to the Scarlet root filesystem. See
[Linux Rootfs Deployment](deployment.md) for details.
