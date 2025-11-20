# Linux Rootfs Deployment

This document describes how to deploy the Linux root filesystem and userspace artifacts into the Scarlet workspace.

## Overview

The deployment process extracts the prebuilt Linux root filesystem (built via Buildroot) and any additional user programs into the Scarlet source tree so they can be included in the final disk image.

The script `tools/linux/deploy_rootfs.sh` handles this process. It extracts artifacts from `/opt/prebuilt` (inside the container) to `mkfs/rootfs/system/linux-riscv64` (in your workspace).

## Usage

### 1. Build the Docker Image

Ensure you have the `scarlet-dev` image built, which contains the prebuilt artifacts.

```bash
docker build -t scarlet-dev .
```

### 2. Run the Deployment Script

Run the container with your workspace mounted and execute the deploy script:

```bash
docker run --rm -v "$(pwd)":/workspaces/Scarlet scarlet-dev /workspaces/Scarlet/tools/linux/deploy_rootfs.sh
```

This will:
1. Extract `rootfs.tar` to `mkfs/rootfs/system/linux-riscv64`.
2. Copy binaries from `/opt/prebuilt/bin` to `/usr/bin` in the rootfs.
3. Copy libraries from `/opt/prebuilt/lib` to `/usr/lib` in the rootfs.

### Handling File Ownership

Since the script runs as root inside the container, the deployed files will be owned by root. To preserve your host user's ownership, pass `TARGET_UID` and `TARGET_GID`:

```bash
docker run --rm -v "$(pwd)":/workspaces/Scarlet \
  -e TARGET_UID=$(id -u) \
  -e TARGET_GID=$(id -g) \
  scarlet-dev /workspaces/Scarlet/tools/linux/deploy_rootfs.sh
```

## Deployment Behavior

The script follows this logic:

1. **Rootfs Extraction**: Extracts `/opt/prebuilt/linux-riscv64.tar` to `mkfs/rootfs/system/linux-riscv64`.
   - **Warning**: Existing contents in the target directory are removed.

2. **Binary Deployment**:
   - Files in `/opt/prebuilt/bin/` are copied to `mkfs/rootfs/system/linux-riscv64/usr/bin/`.
   - They are marked as executable.

3. **Library Deployment**:
   - Files in `/opt/prebuilt/lib/` are synced to `mkfs/rootfs/system/linux-riscv64/usr/lib/`.

## Adding Custom Artifacts

To include custom artifacts in the deployment, you can modify the Dockerfile to place them in `/opt/prebuilt`.

Example Dockerfile snippet:

```dockerfile
RUN mkdir -p /opt/prebuilt/bin /opt/prebuilt/usr && \
    cp -a /opt/my-app/bin/my-app /opt/prebuilt/bin/ && \
    cp -a /opt/my-app/lib/. /opt/prebuilt/lib/
```

## Notes

- **Do not bind-mount** `/workspaces/Scarlet` during the `docker build` phase if you want the artifacts to be part of the image itself.
- The deployment step is designed to run at **runtime** when the workspace is mounted.

