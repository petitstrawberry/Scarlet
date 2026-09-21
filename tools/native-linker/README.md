# Native linker guest acceptance

Wild is the build-time linker: it combines object files and static archives into
new ELF executables. `scarlet-ld` is the separate runtime shared-object loader.
The Wild port and its Actions build are maintained in
[scarlet-rust-nix PR #21](https://github.com/petitstrawberry/scarlet-rust-nix/pull/21).

On 2026-09-21, the native Wild binaries from
[Actions run 35554514866](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35554514866)
passed the direct-object and archive link/execute tests on both AArch64 and RV64
Scarlet guests. Each generated program returned 37, and an unresolved strong
symbol was rejected. The [evidence record](evidence/2026-09-21.json) identifies
the exact build, inputs, commands and serial-log hashes. This initial run does
not establish execution of a native rustc. A subsequent
[Rust std guest test](evidence/2026-09-21-rust-std.json) passed on both architectures
using the complete artifact from
[Actions run 35555398664](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35555398664).
Wild, the native probe and all object/rlib inputs were downloaded and checksum
verified; no part of that artifact was rebuilt locally. Each generated Rust
program printed the expected text and returned 37.

Download and verify the `native-linker-TARGET` Actions artifact, then extract its
`native-linker.tar.xz`. Enter the Scarlet development shell and run:

```sh
python3 tools/native-linker/run-qemu.py \
  --arch aarch64 \
  --artifact /absolute/extracted/native-linker \
  --kernel /absolute/native-kernel \
  --bootstrap /absolute/static-init \
  --output /absolute/new-guest-evidence \
  --limine-cache /absolute/existing-limine-cache
```

Use `--arch riscv64` and matching native binaries for RV64. The bootstrap is the
standard static `user/bin` init; it creates a writable overlay and starts the
probe. The harness validates the artifact checksums and native ELF identities,
stages only link inputs, and starts a private QEMU guest with a deadline. It
never stages the host's linked test executables. Only successful guest link and
execution produces `NATIVE_LINKER FULL PASS` and `guest_link_verified: true`.

Newer artifacts additionally capture a tiny Rust std program's actual object
and rlib inputs and rustc's link arguments. The guest probe links these inputs
and requires the generated program to print `SCARLET_NATIVE_LINKER_RUST_OK` and
return 37. This checks the linker with Rust inputs; compilation of the source
still happens in Actions. Native rustc has its own complete compile-and-execute
acceptance under `tools/native-rustc`.

The harness preserves serial logs, the manifest and boot commands. Its current
tmpfs output does not preserve guest executables after shutdown. Use fresh output
directories for every run; prior PASS evidence cannot satisfy a subsequent run.
