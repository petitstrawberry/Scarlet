# Native dynamic-loader smoke test

`run-qemu.py` boots a small fixture root through the real AArch64 or RISC-V64 kernel and
Limine. Both helpers default to AArch64; select RISC-V64 with `--arch riscv64`. Run it inside the Scarlet development shell (`nix develop`). It requires
Python 3, `cargo-scarlet-plugin-limine`, QEMU, the plugin's image tools, and the
`SCARLET_EFI_CODE_ARM64` / `SCARLET_EFI_VARS_ARM64` firmware variables. The `*_EL2`
variants take priority when present. RISC-V64 uses `SCARLET_EFI_CODE_RV64`
and `SCARLET_EFI_VARS_RV64`, plus QEMU's bundled OpenSBI (`-bios default`).

Build the kernel against this checkout, then generate the fixtures with the
built interpreter. The kernel helper creates its own BSP and target directory,
without changing any existing project's configuration or build output. It needs
the Scarlet nightly toolchain and Rust sources (`build-std`); `--offline` is
available when the dependencies are already cached. The interpreter uses the
Scarlet target standard library supplied by the development shell; a stock
upstream Rust toolchain does not contain that target.

```sh
tools/loader-smoke/build-kernel.sh --output target/loader-smoke/kernel
cargo build --manifest-path user/scarlet-ld/Cargo.toml \
  --target aarch64-unknown-scarlet --release \
  --target-dir target/loader-smoke/loader
python3 tools/loader-smoke/build-fixtures.py \
  --arch aarch64 --hash-style both \
  --loader target/loader-smoke/loader/aarch64-unknown-scarlet/release/scarlet-ld \
  --output target/loader-smoke/fixtures
python3 tools/loader-smoke/run-qemu.py \
  --kernel target/loader-smoke/kernel/target/aarch64-unknown-none-elf/debug/scarlet-loader-smoke-kernel \
  --staging target/loader-smoke/fixtures/staging \
  --output target/loader-smoke/qemu
```

The generated staging root contains a native dynamically linked `/init`, the
interpreter at `/system/bin/scarlet-ld`, and shared libraries under `/system/lib`.
The kernel helper enables Limine, networking, and user floating-point/vector
support, and omits the hypervisor and optional loadable modules. Use a freshly
built kernel so the native handoff and memory-protection ABI match the loader.
`build-fixtures.py` also accepts `--hash-style gnu` or `--hash-style sysv`.
For RISC-V64, use separate output directories:

```sh
tools/loader-smoke/build-kernel.sh --arch riscv64 --output target/loader-smoke/kernel-riscv64
cargo build --manifest-path user/scarlet-ld/Cargo.toml \
  --target riscv64gc-unknown-scarlet --release \
  --target-dir target/loader-smoke/loader
python3 tools/loader-smoke/build-fixtures.py \
  --arch riscv64 --hash-style both \
  --loader target/loader-smoke/loader/riscv64gc-unknown-scarlet/release/scarlet-ld \
  --output target/loader-smoke/fixtures-riscv64
python3 tools/loader-smoke/run-qemu.py --arch riscv64 \
  --kernel target/loader-smoke/kernel-riscv64/target/riscv64gc-unknown-none-elf/debug/scarlet-loader-smoke-kernel \
  --staging target/loader-smoke/fixtures-riscv64/staging \
  --output target/loader-smoke/qemu-riscv64
```

To include a real Rust `no_std` shared object, first run:

```sh
python3 tools/loader-smoke/build-rust-dso.py --arch riscv64 --offline \
  --output target/loader-smoke/rust-riscv64
```

Then add
`--rust-library target/loader-smoke/rust-riscv64/staging/system/lib/libsmoke-rust.so`
to `build-fixtures.py`. Use `aarch64` consistently for the AArch64 variant. The
fixture additionally requires `SCARLET_LOADER_RUST_DSO_OK` before its final
success marker. See [the Rust fixture documentation](rust-dso/README.md) for the
isolated target configuration and the C ABI boundary being tested.

The kernel command line is `console=ttyAMA0 init=/init` on AArch64 and
`console=ttyS0 init=/init` on RISC-V64. The script uses a single TCG CPU, a
headless virtio GPU, 2 GiB of memory on AArch64 or 4 GiB on RISC-V64 (matching
the kernel test runners), a dedicated boot image, and a private writable copy
of EFI variables;
it never opens another project's disk image. A sorted `newc` archive preserves
file modes and symlinks while normalizing ownership and timestamps. Staging is
read-only, and the output directory must be outside it.

Success requires the exact serial line `SCARLET_LOADER_SMOKE_OK`. A
`SCARLET_LOADER_SMOKE_FAIL` marker, interpreter error, guest panic, QEMU exit before the marker, or the default 120-second timeout fails the run.
The script terminates its own QEMU after success or failure. Inspect `serial.log`,
`result.json`, `commands.json`, and `image-build.log` under the output directory.
The QEMU exit code can reflect this deliberate termination even on success;
`result.json` records the marker-based result.

Use a separate output directory per concurrent run. `--timeout SECONDS` changes
the guest deadline; `--prepare-only` builds the archive and boot image without
starting QEMU. The image plugin may download Limine on its first run. For an
offline run, `--limine-cache /path/to/existing/cache` copies an existing cache
(e.g. a directory containing `limine-12.4.0`) into this run's output first.

This test proves only the behaviors exercised by `/init` and its fixture
libraries. It does not establish that rustc, LLVM, TLS, or arbitrary shared
libraries run on Scarlet.
