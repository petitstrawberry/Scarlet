# Scarlet-native Rust toolchain bundle

This bundle installs the versioned Scarlet-native Rust release under
`/opt/scarlet/toolchains/rust/v0.1.0-rc.1` and selects it through the `current`
symlink. The archive contains `rustc`, matching target libraries, the Cranelift
backend, `wild`, and `rust-lld`.

`scarlet-ld` is deliberately not part of the release archive. Dynamic Scarlet
executables request `/system/bin/scarlet-ld` through `PT_INTERP`, so
`bundles/base` builds and installs the matching loader independently on AArch64
and RISC-V64.

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
