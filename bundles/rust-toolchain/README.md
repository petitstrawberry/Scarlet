# Scarlet-native Rust toolchain bundle

This bundle installs the versioned Scarlet-native Rust release under
`/opt/scarlet/toolchains/rust/v0.1.0-dev.825856ca4819` and selects it through the `current`
symlink. The archive contains `rustc`, matching target libraries, the Cranelift
backend, `wild`, and `rust-lld`.

The bundle definition and `current` symlink are imported together from the
[published toolchain bundle](https://github.com/petitstrawberry/scarlet-rust-nix/releases/tag/v0.1.0-dev.825856ca4819).
The selected version and both architecture checksums remain pinned until an
explicit update.

`scarlet-ld` is deliberately not part of the release archive. New dynamic
Scarlet executables request `/bin/scarlet-ld` through `PT_INTERP`, so
`bundles/base` builds and installs the matching loader independently on AArch64
and RISC-V64. The selected compiler release uses this `/bin/scarlet-ld` layout.

The full distribution includes this toolchain bundle. Interactive shells add
`/opt/scarlet/toolchains/rust/current/bin` to `PATH`, making the following
commands available:

```sh
rustc -Vv
wild --version
rust-lld --version
```

This release does not contain Cargo. Cargo, procedural macros, and compiler
self-hosting remain separate acceptance milestones.
