# Buildroot Userspace Bundle

The Buildroot bundle packages the published Buildroot Linux userspace into
Scarlet images.
Public Scarlet architecture names remain `aarch64` and `riscv64`.

The bundle consumes the release archives from
[`scarlet-bundle-linux`](https://github.com/petitstrawberry/scarlet-bundle-linux).
`bundle.toml` pins the architecture-specific archives and their checksums,
including the Mozc runtime, so the generated rootfs is reproducible
without storing the Linux userspace tree in this repository.

The local Buildroot scripts below are still available when rebuilding or
debugging the producer artifacts; they are not the source used by a clean
Buildroot bundle build. The default `full` bundle uses Debian userspace from
`bundles/debian` instead.

The `full-buildroot` bundle combines Buildroot userspace with the desktop,
experimental apps, and Scarlet Rust toolchain. The RISC-V full project selects
this composition; AArch64 projects can also select it.

## Layout

- Local deployed rootfs: `rootfs/systems/linux-${ARCH}`
- Buildroot tarball: `prebuilt/${ARCH}/rootfs.tar`
- Generated executable artifacts: `prebuilt/${ARCH}/bin`
- Optional staged overlays: `prebuilt/${ARCH}/root`, `lib`, and `share`

Run `tools/prepare.sh` to build and deploy artifacts, or run its individual
helper scripts on a Linux host. Generated source and build trees are kept under
the ignored `cache/` directory, while staged artifacts are kept under the
ignored `prebuilt/` directory until `deploy_rootfs.sh` installs them into the
bundle rootfs tree.
