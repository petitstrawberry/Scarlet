# Linux ABI Demo

This document describes how to run the Linux ABI demo on Scarlet.

## Overview

Scarlet provides a partial Linux ABI implementation that allows running simple Linux userspace binaries.
The demo environment includes a Buildroot-based root filesystem providing standard utilities via **BusyBox**, along with sample applications like `green` and `fbdoom`.

## Prerequisites

- You must be running inside the `scarlet-dev` Docker container.
- The kernel must be built and running (see main README).

## Running the Demo

### 1. Build/Prepare Userspace Artifacts

If you haven't already built the Linux userspace artifacts (rootfs and demo binaries), run the following commands in the `scarlet-dev` container:

```bash
# Build the Buildroot rootfs
bash tools/linux/build_buildroot.sh

# Build demo programs (green, fbdoom)
bash tools/linux/build_user_programs.sh
```

These scripts will place the necessary files in `/opt/prebuilt`.

### 2. Deploy Artifacts to Scarlet Rootfs

The build scripts create artifacts, but they need to be deployed into Scarlet's root filesystem structure before building the disk image.
For detailed deployment options, see [Linux Rootfs Deployment](deployment.md).

```bash
# Deploy the Linux rootfs and binaries to mkfs/rootfs/system/linux-riscv64
bash tools/linux/deploy_rootfs.sh
```

### 3. Build and Run Scarlet

Build the kernel and the root filesystem image, then run Scarlet:

```bash
cargo make build-riscv64
cargo make run-riscv64
```

### 4. Execute Linux Binaries

Once dropped into the Scarlet shell, you can execute the Linux binaries. The Linux root filesystem is located at `/system/linux-riscv64`.

**Basic Utilities (BusyBox):**

You can run standard Linux commands provided by BusyBox:

```bash
# List files in the Linux rootfs
/system/linux-riscv64/bin/busybox ls -l /system/linux-riscv64

# Print working directory
/system/linux-riscv64/bin/busybox pwd

# Cat a file
/system/linux-riscv64/bin/busybox cat /system/linux-riscv64/etc/passwd
```

**Advanced Demos:**

Other demo binaries are available but require specific arguments or setup:

- **green**: A SDL-based PDF rendering test (requires a PDF file argument).
- **fbdoom**: A Doom port (requires a WAD file, e.g., `fbdoom -iwad /path/to/doom1.wad`).

```bash
# Example: Running fbdoom (if you have a WAD file)
/system/linux-riscv64/usr/bin/fbdoom -iwad /system/linux-riscv64/usr/share/games/doom/doom1.wad
```

## Environment Configuration

The Scarlet shell environment (`.shrc`) automatically sets `LD_LIBRARY_PATH` to ensure dynamic linking works for Linux binaries:

```bash
export LD_LIBRARY_PATH=/scarlet/system/linux-riscv64/usr/lib:/scarlet/system/linux-riscv64/lib
```

This path points to the libraries within the Linux rootfs (accessed via the `/scarlet` gateway mount). If you need to use `LD_PRELOAD`, ensure the paths are also absolute and accessible within the Linux ABI environment.

## Troubleshooting

If binaries fail to run:
- Check [`docs/abi/linux/status.md`](status.md) to see if required syscalls are supported.
- Ensure the rootfs is correctly mounted.
- Verify that the binaries were built for RISC-V 64-bit (riscv64).
