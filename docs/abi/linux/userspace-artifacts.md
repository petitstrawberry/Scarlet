# Linux Userspace Artifacts

This document explains how to rebuild the Buildroot-generated root filesystem and optional Linux user-space binaries (`green`, `fbdoom`) that power Scarlet's partial Linux ABI support inside the development container.

## Prerequisites

- Run inside the `scarlet-dev` container (see [`README.md`](../../../README.md) for entering the environment).
- Buildroot sources must already be unpacked under `/opt/buildroot` (handled automatically by the Dockerfile).

## Building Buildroot

Use the helper script to build the root filesystem:

```bash
bash tools/linux/build_buildroot.sh
```

The script performs:

- `make -j $(nproc)` inside `/opt/buildroot`.
- Packaging of the resulting `rootfs.tar` into `/opt/prebuilt/linux-riscv64.tar`.
- Regeneration of the `linux-riscv64/` directory under `output/images`.

If either `/opt/buildroot` or `rootfs.tar` is missing the script prints an error and exits.

## Building Optional User Programs

With the Buildroot toolchain ready, build the auxiliary binaries:

```bash
bash tools/linux/build_user_programs.sh
```

This script:

- Verifies the presence of the Buildroot cross toolchain (`output/host/bin/riscv64-buildroot-linux-musl-gcc`).
- Clones or updates the required repositories (`green`, `fbdoom`).
- Builds each project using the Buildroot toolchain.
- Installs the resulting binaries and any required libraries into `/opt/prebuilt/bin` and `/opt/prebuilt/lib`.

If the toolchain is missing, run `tools/linux/build_buildroot.sh` first.

## Customisation Tips

- Override directories via environment variables when needed:
  - `BUILDROOT_DIR`, `PREBUILT_DIR`, `WORKDIR` etc. are respected by both scripts.
- Both scripts exit on the first error (`set -euo pipefail`) so failures are surfaced early.

## Common Workflow

1. `bash tools/linux/build_buildroot.sh`
2. `bash tools/linux/build_user_programs.sh`

This matches the behaviour previously baked into the Docker image while keeping the image slim.

## Deployment

After building artifacts, they must be deployed to the Scarlet root filesystem. See [Linux Rootfs Deployment](deployment.md) for details.
