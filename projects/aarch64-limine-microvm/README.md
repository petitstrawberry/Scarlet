# AArch64 Limine MicroVM

This project boots Scarlet with the built-in AArch64 hypervisor enabled, then
starts Firecracker from the Scarlet userspace rootfs.

The project is intended to be driven primarily by `cargo-scarlet`: the boot
image, initramfs, rootfs staging, and microVM guest artifacts are selected from
Scarlet layer metadata and executed through a generated image plan. Avoid adding
new dependencies on the repository top-level `cargo make` flow; keep new
image-building logic in layer recipes, this project's helper tools, or
`cargo-scarlet` plan generation.

## First-time setup

Run these commands from the repository root in the development environment.
This project does not require the top-level `cargo make` tasks. Use the
project-local helper scripts for external artifacts, then use `cargo-scarlet`
for project build, image, and run operations.

```sh
export BUILDROOT_DIR="$PWD/.scarlet/cache/buildroot-aarch64"
export PREBUILT_DIR="$PWD/.scarlet/cache/prebuilt"
export WORKDIR="$PWD/.scarlet/cache"

ARCH=aarch64 bash tools/linux/build_buildroot.sh
ARCH=aarch64 bash tools/linux/build_guest_image.sh
projects/aarch64-limine-microvm/tools/prepare_prebuilt.sh

(cd user/lib && cargo make build-userlib-release-aarch64)
(cd user/bin && cargo make build-release-aarch64)

cargo run --manifest-path cargo-scarlet/Cargo.toml -- image \
  --machine qemu-aarch64-virt \
  --distro scarlet-microvm \
  --image host \
  --release

SCARLET_QEMU_ACCEL=tcg \
SCARLET_QEMU_VIRTUALIZATION=1 \
SCARLET_QEMU_SMP=1 \
cargo run --manifest-path cargo-scarlet/Cargo.toml -- run \
  --project projects/aarch64-limine-microvm \
  --release
```

`ARCH=aarch64 bash tools/linux/build_buildroot.sh` builds the AArch64 Buildroot
toolchain and rootfs tarball under `$BUILDROOT_DIR` and
`$PREBUILT_DIR/aarch64`.

`build_guest_image.sh` builds the guest Linux kernel and initramfs into
`$PREBUILT_DIR/aarch64/bin`.

`prepare_prebuilt.sh` copies the guest artifacts into
`projects/aarch64-limine-microvm/prebuilt` and downloads Firecracker if the
project-local copy is missing. The actual binaries under `prebuilt/system/` are
ignored by git.

After this setup, `cargo-scarlet image --machine qemu-aarch64-virt --distro
scarlet-microvm --image host` uses the project-local prebuilt artifacts and does
not need to fetch Firecracker during image creation. The distro selects
`limine-uefi-aarch64` as its default boot target; pass `--boot
limine-uefi-aarch64` only when overriding that choice explicitly.

If `mkfs/rootfs/system/linux-aarch64/usr/bin/guest-Image` and
`guest-initramfs.cpio.gz` already exist, `prepare_prebuilt.sh` can copy from
there without rebuilding Buildroot.

The Linux helper scripts still default to `/opt/buildroot-aarch64` and
`/opt/prebuilt` for compatibility with the existing Docker workflow. For local
development, prefer the repository-local environment variables shown above.
Buildroot artifact generation is Linux-host work: run it in `scarlet-dev`, a
Linux VM, or a Linux Nix shell. macOS can build Scarlet itself, but these
Buildroot/userland artifact scripts intentionally stop there with guidance.

The top-level `cargo make image-aarch64-microvm` and
`cargo make run-aarch64-microvm` tasks remain compatibility shortcuts, but new
microVM documentation should prefer the explicit project-local flow above.

## Project-local Image Flow

The host image recipe lives in `layers/scarlet-reference/images/host.toml`. The
QEMU virtual hardware contract lives in
`layers/scarlet-reference/machines/qemu-aarch64-virt.toml`. The Scarlet microVM
product policy lives in `layers/scarlet-reference/distros/scarlet-microvm.toml`,
and the Limine UEFI boot packaging contract lives in
`layers/scarlet-reference/boot_targets/limine-uefi-aarch64.toml`.
`projects/aarch64-limine-microvm/scarlet-config.toml` is now only the temporary
pre-split kernel build adapter config.

- The Limine boot image is emitted under `.scarlet/images/`.
- The initramfs is assembled from this project's `initramfs/` directory plus the
  image recipe inputs.
- The rootfs is staged under `.scarlet/rootfs-stage/` by
  `tools/build_rootfs_ext2.sh`. The new layer-driven path passes a generated
  install manifest so only selected package files are copied from `user/bin`
  outputs.
- MicroVM-specific guest artifacts live under `prebuilt/`; the binary payloads
  under `prebuilt/system/` are intentionally ignored by git.

Current compatibility inputs still come from repository-wide build outputs, but
the locations are declared in the layer image recipe:

- `user/bin/dist/aarch64`
- `mkfs/rootfs`
- `mkfs/dist/modules/aarch64-unknown-none-elf`

When moving more build steps local, prefer replacing those inputs with explicit
project-local helper scripts or `cargo-scarlet` recipe steps rather than adding
more top-level `cargo make` dependencies.
