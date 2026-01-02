# Linux Rootfs Deployment

This document describes how to deploy the Linux root filesystem and userspace artifacts into the Scarlet workspace.

## Overview

The deployment process extracts the prebuilt Linux root filesystem (built via Buildroot) and any additional user programs into the Scarlet source tree so they can be included in the final disk image.

The script `tools/linux/deploy_rootfs.sh` handles this process. It extracts artifacts from `/opt/prebuilt` (inside the container) to `mkfs/rootfs/system/linux-{ARCH}` (in your workspace).

## Supported Architectures

- **riscv64** - RISC-V 64-bit (default)
- **aarch64** - ARM 64-bit (AArch64)

## Prerequisites

Before running the deployment script, you must ensure that the Linux userspace artifacts (Buildroot rootfs and demo binaries) have been built and are available in `/opt/prebuilt` inside the container.

If you haven't built them yet, please refer to [Linux Userspace Artifacts](userspace-artifacts.md) for instructions.

## Usage

### 1. Start the Development Container

Start the `scarlet-dev` container with your workspace mounted:

```bash
docker run -it --rm -v "$(pwd)":/workspaces/Scarlet scarlet-dev
```

### 2. Build Artifacts (Inside Container)

**Note:** The `scarlet-dev` Docker image does **not** contain prebuilt artifacts to keep the image size manageable. You must build them inside the container before deployment.

#### For RISC-V 64 (default):

```bash
# Build rootfs and user programs
bash tools/linux/build_buildroot.sh
bash tools/linux/build_user_programs.sh
```

#### For AArch64:

```bash
# Build rootfs and user programs with ARCH=aarch64
ARCH=aarch64 bash tools/linux/build_buildroot.sh
ARCH=aarch64 bash tools/linux/build_user_programs.sh
```

### 3. Deploy (Inside Container)

Once built, deploy them to the workspace:

#### For RISC-V 64:

```bash
bash tools/linux/deploy_rootfs.sh
# Or explicitly:
ARCH=riscv64 bash tools/linux/deploy_rootfs.sh
```

This will extract to `mkfs/rootfs/system/linux-riscv64`.

#### For AArch64:

```bash
ARCH=aarch64 bash tools/linux/deploy_rootfs.sh
```

This will extract to `mkfs/rootfs/system/linux-aarch64`.

The deployment script will:
1. Extract `linux-{ARCH}.tar` to `mkfs/rootfs/system/linux-{ARCH}`.
2. Copy binaries from `/opt/prebuilt/bin` to `/usr/bin` in the rootfs.
3. Copy libraries from `/opt/prebuilt/lib` to `/usr/lib` in the rootfs.

### Handling File Ownership

If you are running the container as root (default), the deployed files will be owned by root. To preserve your host user's ownership, you can pass environment variables when starting the container, or set them before running the deploy script:

```bash
# Example: Running deploy with ownership fix
export TARGET_UID=$(id -u)
export TARGET_GID=$(id -g)
bash tools/linux/deploy_rootfs.sh
```

## Deployment Behavior

The script follows this logic:

1. **Rootfs Extraction**: Extracts `/opt/prebuilt/linux-{ARCH}.tar` to `mkfs/rootfs/system/linux-{ARCH}`.
   - **Warning**: Existing contents in the target directory are removed.

2. **Binary Deployment**:
   - Files in `/opt/prebuilt/bin/` are copied to `mkfs/rootfs/system/linux-{ARCH}/usr/bin/`.
   - They are marked as executable.

3. **Library Deployment**:
   - Files in `/opt/prebuilt/lib/` are copied to `mkfs/rootfs/system/linux-{ARCH}/usr/lib/`.

The ARCH environment variable controls which architecture's artifacts are deployed (defaults to `riscv64` if not specified).

## Adding Custom Artifacts

To include custom artifacts in the deployment, you can modify the Dockerfile to place them in `/opt/prebuilt`.

Example Dockerfile snippet:

```dockerfile
RUN mkdir -p /opt/prebuilt/bin /opt/prebuilt/usr && \
    cp -a /opt/my-app/bin/my-app /opt/prebuilt/bin/ && \
    cp -a /opt/my-app/lib/. /opt/prebuilt/lib/
```

## Directory Structure

After deployment, the directory structure will be:

```
mkfs/
  rootfs/
    system/
      linux-riscv64/      # RISC-V 64 rootfs (if deployed)
        bin/
        etc/
        lib/
        usr/
        ...
      linux-aarch64/      # AArch64 rootfs (if deployed)
        bin/
        etc/
        lib/
        usr/
        ...
```

Each architecture has its own isolated directory tree.

## Building the Final Image

Once the rootfs is deployed, you can build the final Scarlet disk image with:

```bash
# For RISC-V 64
cargo make build

# For AArch64
ARCH=aarch64 cargo make build
```

The generated ext2 image includes the deployed Linux rootfs for the selected architecture.

## Notes

- **Do not bind-mount** `/workspaces/Scarlet` during the `docker build` phase if you want the artifacts to be part of the image itself.
- The deployment step is designed to run at **runtime** when the workspace is mounted.
- Both architectures can be deployed simultaneously by running the deployment script for each architecture.

