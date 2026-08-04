# Linux ABI Bundle

The Linux bundle packages the published Linux userspace into Scarlet images.
Public Scarlet architecture names remain `aarch64` and `riscv64`.

The standard full-image manifest consumes the pinned runtime manifest from
[`scarlet-bundle-linux`](https://github.com/petitstrawberry/scarlet-bundle-linux).
That manifest selects the architecture-specific release archive and its
checksum, including the Mozc runtime, so the generated rootfs is reproducible
without storing the Linux userspace tree in this repository.

The local Buildroot scripts below are still available when rebuilding or
debugging the producer artifacts; they are not the source used by a clean
standard full-image build.

## Layout

- Local deployed rootfs: `rootfs/system/linux-${ARCH}`
- Buildroot tarball: `prebuilt/${ARCH}/rootfs.tar`
- Generated executable artifacts: `prebuilt/${ARCH}/bin`
- Optional staged overlays: `prebuilt/${ARCH}/root`, `lib`, and `share`

Run `tools/prepare.sh` to build and deploy artifacts, or run its individual
helper scripts on a Linux host. Generated source and build trees are kept under
the ignored `cache/` directory, while staged artifacts are kept under the
ignored `prebuilt/` directory until `deploy_rootfs.sh` installs them into the
bundle rootfs tree.
