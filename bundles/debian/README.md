# Debian Userspace Bundle

This bundle installs Debian trixie userspace and Scarlet compatibility overlays.
The default `full` bundle combines it with the desktop, experimental apps, and
Scarlet Rust toolchain. The former `full-debian` composition is now `full`;
its Debian-specific overlays live here.

The release archive and checksum are pinned to `scarlet-bundle-debian` v0.3.26.
This release supports AArch64 only. Other architectures fail checksum selection
until a matching archive and checksum are added to `bundle.toml`.

The Linux ABI backing root remains `/systems/linux-{arch}`.
