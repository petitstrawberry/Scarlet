# Native rust-analyzer

The AArch64 full images install `/bin/rust-analyzer`, compatible Rust library
sources, and their offline registry dependencies from a published, SHA-256-pinned
release. This bundle follows `rust-toolchain` in the full, Alpine, and Buildroot
compositions. Kureha discovers Cargo and rustc without sourcing an interactive
shell profile. No sibling checkout or local dependency override is required.

The selected Rust toolchain is `v0.1.0-dev.79f356924d1a`. The server is built from
`petitstrawberry/rust` commit `d12e1fb78b2c1dbcbf08820c73767691cb97a2c6` with that
checkout's native standard library. Its `Command::spawn` uses Scarlet's existing
Environment API and transfers only configured standard streams. The installed
compiler and Cargo remain the published `79f356` binaries; their compiled standard
library is not replaced by this server package. The supplied library sources
contain the same public API plus the server revision's Scarlet runtime fixes.

Native acceptance uses Kureha's production stdio transport and the real server.
It checks Cargo metadata for an unconventional binary target and local dependency,
hover, exact completion edits, definitions in unopened dependencies and the real
standard library, unsaved document changes, and clean shutdown. It does not
substitute protocol responses or skip failed sysroot loading.

The diskless QEMU fixture uses 4 GiB RAM, including its approximately 466 MiB
initramfs; 2 GiB runs exhausted physical memory. This is not a performance
benchmark or proof of a minimum memory requirement on the normal ext2 root.
Scarlet commits `10203875` and `c08f7901` bound read bounce buffers and remove
per-page duplicate anonymous zero backing found during these runs.

Procedural macros and rustfmt are not supplied. Compiler check-on-save and the
final native GUI interaction are not covered by the protocol acceptance.
Scarlet disables initial cache priming and proc macros. Pending-edit Salsa
cancellation waits for existing snapshots instead of unwinding under
`panic=abort`; long analysis can therefore delay edits.

RV64 has a cross-built release artifact but no native LSP acceptance yet, so this
bundle installs the server on AArch64 only. The RV64 kernel test run stopped in
`device::graphics::tests::integration_tests` at framebuffer presentation; it is
not recorded as passing. AArch64 kernel regressions cover zero backing, private
fork data, partial unmap, raw syscall output translation, eager physical fallback,
and allocation failure.

Reproduction: build `tools/native-lsp-probe` from Kureha commit
`6c6fc53e4137f4729fb6f6883efe44c27d2efc24`, stage the published toolchain and this
archive with `/init` and `/bin/scarlet-ld`, then use
`tests/native-desktop-smoke/run.py --memory 4G` with the probe launch program.
The helper's README documents its required startup environment and checks.
Release assets include the exact native run commands, serial evidence, result,
archive checksums, and independent binary/source manifests.

Final packaged AArch64 acceptance passed in 189.159 seconds under QEMU TCG
on Scarlet `c08f7901`. Evidence: [published release](https://github.com/petitstrawberry/rust/releases/tag/scarlet-rust-analyzer-d12e1fb78b2).

The AArch64 full image was successfully rebuilt with
`cargo scarlet image --project projects/aarch64-limine-full --release` in the
pinned Nix development shell, including the published server hash and Cargo.
