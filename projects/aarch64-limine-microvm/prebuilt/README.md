# AArch64 MicroVM Prebuilt Artifacts

This directory contains project-local artifacts used by the AArch64 microVM
image builder. Keeping these files here avoids depending on transient files in
`mkfs/rootfs` or on network downloads during normal image creation.

- `system/linux-aarch64/usr/bin/firecracker`: Firecracker v1.15.1 aarch64.
- `system/linux-aarch64/usr/bin/guest-Image`: AArch64 Linux guest kernel.
- `system/linux-aarch64/usr/bin/guest-initramfs.cpio.gz`: AArch64 guest initramfs.

Refresh `guest-Image` and `guest-initramfs.cpio.gz` from
`bundles/linux/tools/build_guest_image.sh` output when the guest payload changes.

Run `../tools/prepare_prebuilt.sh` from this project, or
`projects/aarch64-limine-microvm/tools/prepare_prebuilt.sh` from the repository
root, to populate the ignored artifact tree. The script copies guest artifacts from
`mkfs/rootfs/system/linux-aarch64/usr/bin` or `/opt/prebuilt/aarch64/bin`, and
downloads Firecracker only when `firecracker` is missing.

For local development, set `PREBUILT_DIR` when running the Linux guest builders
and set `SCARLET_MICROVM_GUEST_BIN_DIR` when preparing this directory if the
guest artifacts are not in `/opt/prebuilt/aarch64/bin`.
