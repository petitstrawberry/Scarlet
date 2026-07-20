# Linux ABI Bundle

The Linux bundle packages a deployed Buildroot root filesystem into Scarlet
images. Public Scarlet architecture names remain `aarch64` and `riscv64`.

## Layout

- Deployed rootfs: `rootfs/system/linux-${ARCH}`
- Buildroot tarball: `prebuilt/${ARCH}/rootfs.tar`
- Generated executable artifacts: `prebuilt/${ARCH}/bin`
- Optional staged overlays: `prebuilt/${ARCH}/root`, `lib`, and `share`

Run `tools/prepare.sh` to build and deploy artifacts, or run its individual
helper scripts on a Linux host. Generated source and build trees are kept under
the ignored `cache/` directory, while staged artifacts are kept under the
ignored `prebuilt/` directory until `deploy_rootfs.sh` installs them into the
bundle rootfs tree.
