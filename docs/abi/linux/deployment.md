# Linux Rootfs Deployment

This document describes how to deploy the Linux root filesystem and userspace
artifacts into the Scarlet workspace.

## Overview

`bundles/linux/tools/deploy_rootfs.sh` extracts a Buildroot `rootfs.tar` and overlays
additional prebuilt programs into `bundles/linux/rootfs/linux-$ARCH`, which is the
tree included in the final Scarlet disk image.

Buildroot generation should be done on Linux. Deployment itself only needs the
staged artifacts and can use an injected `PREBUILT_DIR`.

## Supported Architectures

- `riscv64` - RISC-V 64-bit (default)
- `aarch64` - ARM 64-bit (AArch64)

## Building Artifacts

See [Linux Userspace Artifacts](userspace-artifacts.md). For AArch64 with
repository-local paths on a Linux host:

```bash
export BUILDROOT_DIR="$PWD/.scarlet/cache/buildroot-aarch64"
export PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt"
export WORKDIR="$PWD/.scarlet/cache"

ARCH=aarch64 bash bundles/linux/tools/build_buildroot.sh
ARCH=aarch64 bash bundles/linux/tools/build_user_programs.sh
```

The deployment script expects:

- `$PREBUILT_DIR/$ARCH/rootfs.tar`
- optional binaries in `$PREBUILT_DIR/$ARCH/bin`
- optional libraries in `$PREBUILT_DIR/$ARCH/lib`

## Deploying

For RISC-V 64:

```bash
bash bundles/linux/tools/deploy_rootfs.sh
```

For AArch64:

```bash
ARCH=aarch64 \
PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt" \
bash bundles/linux/tools/deploy_rootfs.sh
```

The script extracts to `bundles/linux/rootfs/linux-$ARCH`, then copies staged
binaries into `usr/bin`, staged libraries into `usr/lib`, staged data into
`usr/share`, and any staged root overlay into the extracted root.

Existing contents in the target directory are removed before extraction.

## File Ownership

If you are running inside a container as root, set ownership variables before
deploying:

```bash
TARGET_UID="$(id -u)" \
TARGET_GID="$(id -g)" \
ARCH=aarch64 \
PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt" \
bash bundles/linux/tools/deploy_rootfs.sh
```

## Final Image

Once the rootfs is deployed, build the Scarlet image for the same architecture:

```bash
cargo make build-aarch64
```

or:

```bash
cargo make build-riscv64
```
