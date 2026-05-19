# AArch64 Limine MicroVM

This project boots Scarlet with the built-in AArch64 hypervisor enabled, then
starts Firecracker from the Scarlet userspace rootfs.

The project is intended to be driven primarily by `cargo-scarlet`: the boot
image, initramfs, rootfs staging, and microVM guest artifacts should be described
by this project's recipe and helper tools. Avoid adding new dependencies on the
repository top-level `cargo make` flow; keep new image-building logic in this
project or in `cargo-scarlet` project recipes.

## First-time setup

Run these commands from the repository root in the development environment.
This project does not require the top-level `cargo make` tasks. Use the
project-local helper scripts for external artifacts, then use `cargo-scarlet`
for project build, image, and run operations.

```sh
export BUILDROOT_DIR="$PWD/.scarlet/cache/buildroot-aarch64"
export PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt"
export WORKDIR="$PWD/.scarlet/cache"

bash tools/linux/build_buildroot_aarch64.sh
ARCH=aarch64 bash tools/linux/build_guest_image.sh
projects/aarch64-limine-microvm/tools/prepare_prebuilt.sh

(cd user/lib && cargo make build-userlib-release-aarch64)
(cd user/bin && cargo make build-release-aarch64)

cargo run --manifest-path cargo-scarlet/Cargo.toml -- image \
  --project projects/aarch64-limine-microvm \
  --release

SCARLET_QEMU_ACCEL=tcg \
SCARLET_QEMU_VIRTUALIZATION=1 \
SCARLET_QEMU_SMP=1 \
cargo run --manifest-path cargo-scarlet/Cargo.toml -- run \
  --project projects/aarch64-limine-microvm \
  --release
```

`build_buildroot_aarch64.sh` builds the AArch64 Buildroot toolchain and rootfs
tarball under `$BUILDROOT_DIR` and `$PREBUILT_DIR/aarch64`.

`build_guest_image.sh` builds the guest Linux kernel and initramfs into
`$PREBUILT_DIR/aarch64/bin`.

`prepare_prebuilt.sh` copies the guest artifacts into
`projects/aarch64-limine-microvm/prebuilt` and downloads Firecracker if the
project-local copy is missing. The actual binaries under `prebuilt/system/` are
ignored by git.

After this setup, `cargo-scarlet image` and `cargo-scarlet run` use the
project-local prebuilt artifacts and do not need to fetch Firecracker during
image creation.

If `mkfs/rootfs/system/linux-aarch64/usr/bin/guest-Image` and
`guest-initramfs.cpio.gz` already exist, `prepare_prebuilt.sh` can copy from
there without rebuilding Buildroot.

The Linux helper scripts still default to `/opt/buildroot-aarch64` and
`/opt/prebuilt` for compatibility with the existing Docker workflow. For local
Nix/macOS development, prefer the repository-local environment variables shown
above.

The top-level `cargo make image-aarch64-microvm` and
`cargo make run-aarch64-microvm` tasks remain compatibility shortcuts, but new
microVM documentation should prefer the explicit project-local flow above.

## Project-local Image Flow

The image recipe lives in `scarlet-config.toml`.

- The Limine boot image is emitted under `.scarlet/images/`.
- The initramfs is assembled from this project's `initramfs/` directory plus the
  project recipe inputs.
- The rootfs is staged under `.scarlet/rootfs-stage/` by
  `tools/build_rootfs_ext2.sh`.
- MicroVM-specific guest artifacts live under `prebuilt/`; the binary payloads
  under `prebuilt/system/` are intentionally ignored by git.

Current compatibility inputs still come from repository-wide build outputs, but
the locations are declared in `scarlet-config.toml`:

- `user/bin/dist/aarch64`
- `mkfs/rootfs`
- `mkfs/dist/modules/aarch64-unknown-none-elf`

When moving more build steps local, prefer replacing those inputs with explicit
project-local helper scripts or `cargo-scarlet` recipe steps rather than adding
more top-level `cargo make` dependencies.
