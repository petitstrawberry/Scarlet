# Scarlet-native Rust toolchain bundle

This bundle installs the versioned Scarlet-native Rust release under
`/opt/scarlet/toolchains/rust/v0.1.0-rc.1` and selects it through the `current`
symlink. The archive contains `rustc`, matching target libraries, the Cranelift
backend, `wild`, and `rust-lld`.

`scarlet-ld` is deliberately not part of the release archive. New dynamic
Scarlet executables request `/bin/scarlet-ld` through `PT_INTERP`, so
`bundles/base` builds and installs the matching loader independently on AArch64
and RISC-V64. The bundled `v0.1.0-rc.1` compiler still requests the former
`/system/bin/scarlet-ld` path and must be replaced before it can run in an
image with the new loader layout.

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
