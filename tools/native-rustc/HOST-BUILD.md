# Native compiler build recipe for Actions

`prepare_host_build.py` writes a config and JSON command. It does not compile,
fetch dependencies, modify the Rust source, or change installed toolchains.
Execute the emitted command in **scarlet-rust-nix Actions**; heavy local builds
are outside the current workflow.

The recipe is based on Rust source
`39c689a4859b9d8ee1828720135defd125c03d31`. First prepare an isolated, writable copy
with the target/std/libloading changes and native dependency patches. For a
vendored Nix source, preserve or regenerate Cargo's checksums and lockfile before
enabling `--vendor`.

```sh
python3 tools/native-rustc/prepare_host_build.py \
  --rust-source "$PREPARED_RUST_SOURCE" \
  --stage0-rustc "$STAGE0/bin/rustc" \
  --stage0-cargo "$STAGE0/bin/cargo" \
  --host-llvm-config "$HOST_LLVM/bin/llvm-config" \
  --native-cc "$CROSS_WRAPPERS/cc" \
  --native-cxx "$CROSS_WRAPPERS/cxx" \
  --native-ar "$CROSS_TOOLS/bin/llvm-ar" \
  --native-linker "$CROSS_TOOLS/bin/ld.lld" \
  --target aarch64-unknown-scarlet --backend dummy \
  --build-dir "$NATIVE_BUILD_CACHE" --output "$FRESH_RECIPE_DIR" \
  --vendor --offline --nix
```

Supported stage0 build hosts are x86_64 Linux, AArch64 Linux, and AArch64 macOS.
The host is detected with `rustc -Vv`; no host compiler is executed in Scarlet.
`--native-cc` and `--native-cxx` must cross-compile for the target architecture
without accidentally including the build host's libc. LLVM tools can be reused
from the existing build-host toolchain; a native Scarlet LLVM is unnecessary for
this path. Pass `--host-llvm-has-rust-patches` only for a Rust-patched host LLVM.
`--skip-stage0-validation` explicitly permits an alternative compiler version;
otherwise bootstrap validates it against `src/stage0`. Pass `--stage0-rustfmt`
to reuse an available formatter and avoid bootstrap downloading a separate
rustfmt toolchain. An executable sibling of `--stage0-rustc` is detected
automatically when present.

The output `build-command.json` contains `cwd`, `argv`, `env`, the detected host,
and exact packaging paths. An Actions wrapper can load it and call
`subprocess.run(recipe["argv"], cwd=recipe["cwd"], env={**os.environ,
**recipe["env"]}, check=True)`. The argv is an array so paths need no shell
interpolation. The source is never patched by this generator.

## Why stage2 and why dummy works

The sequence is:

1. Build a fresh development-host stage1 compiler with the updated target specs,
   using the already available host LLVM via `llvm-config`.
2. Use that compiler to build native Scarlet std, rustc_driver and rustc-main.
3. Assemble the Scarlet stage2 compiler, with no LLVM feature/dependency in its
   own compiler graph.

`[rust] codegen-backends = ["llvm"]` controls the build-host stage1 compiler.
`[target.<scarlet>] codegen-backends = ["dummy"]` overrides only the native
compiler. Bootstrap parses dummy as `Custom("dummy")`, emits a warning, and
stores `CFG_DEFAULT_CODEGEN_BACKEND=dummy`. The assembly loop explicitly skips
custom backend building, and rustc already implements the dummy backend. No
`rustc_codegen_dummy` crate or bootstrap enum patch is required.

Use `x build compiler/rustc --stage 2`; **`x check` forcibly enables the LLVM
feature** in the audited `rustc_features` code, even when the target backend list
does not contain LLVM. `codegen-backends = []` is rejected; it is not needed.
The existing stage1 compiler is not reused as the patched cross compiler: its
built-in targets still lack the new dynamic-link options.

The generator disables native RPATH generation. Otherwise bootstrap assumes a
C linker driver and inserts `-Wl,-z,origin`/`-Wl,-rpath,...`, which are unsuitable
for Scarlet's direct LLD link. It also disables bundled LLD, LLVM tools, jemalloc,
docs, profilers, sanitizers, and full bootstrap. It keeps std static in
rustc_driver through bootstrap's existing policy. Target-only flags select
panic abort and 4 KiB ELF maximum pages. `--allow-shlib-undefined` lets the main
executable link against rustc_driver's runtime `dl*` imports, which the Scarlet
interpreter provides. Undefined references from executable object files still
fail to link; this is narrower than suppressing all undefined-symbol errors.

The production native-host pipeline uses `--backend cranelift`. Bootstrap builds
the native Cranelift backend against the matching compiler artifacts and installs
it into the native sysroot. Its default feature set does not include JIT. Backend
selection alone is not proof of a working port, so the downloadable artifact is
also subjected to the guest compile/link/run acceptance described in
[`docs/development/native-rustc.md`](../../docs/development/native-rustc.md).

## Packaging the matching std

`compiler/rustc --stage2` builds native std as a prerequisite, but its assembly
step does not populate all target std rlibs into the final native sysroot.
Copy the contents of the emitted `stdlib_source` directory into
`stdlib_destination` **after the successful build**. These are:

```text
<build>/<BUILD_HOST>/stage1/lib/rustlib/<NATIVE_TARGET>/lib
  -> <build>/<NATIVE_TARGET>/stage2/lib/rustlib/<NATIVE_TARGET>/lib
```

Use these exact artifacts, not the preinstalled cross-toolchain's std. Avoid
adding `library/std --stage2` just to populate that directory: `Std::make_run`
explicitly constructs an additional stage2 build-host compiler before uplifting
the already built stage1 std. Keep compiler/driver/backend hashes and manifests
with the packaged result.

The integrated pipeline and downloadable artifact helper are in the
[scarlet-rust-nix native-host workflow](https://github.com/petitstrawberry/scarlet-rust-nix/actions/workflows/native-host.yml).
Its prepared source includes the CRT separation, native compiler cfg fixes,
Cranelift/target-lexicon changes, libloading adapters, aligned stacker fallback,
named tempfile support, and entropy-required getrandom backend. Their lightweight
checks, native compiler builds, and guest execution have passed for AArch64 and
RV64. Each future artifact still needs the same acceptance before it is marked
guest-verified.

## Preflight and dependency audit

Before dispatching a heavy Actions build, validate patch application, vendor
checksums/lockfile, executable paths and config generation. When a matching
bootstrap executable is already available, it can validate the config with
`build compiler/rustc --src <source> --config <config> --build-dir <isolated-dir>
--stage 2 --dry-run -vv` without compilation. Running `x.py` itself can first
compile bootstrap; leave that path to Actions. The audited local dry-run selected
LLVM for development-host stage1, only `max_level_info` for the native dummy
compiler, and assembled the native stage2 without building LLVM for Scarlet.
This validates build selection, not successful native compilation.

Source-level preflight identifies these additional areas; the CI compile log and
guest runs must establish which patches are sufficient:

| Dependency / compiler code | Native Scarlet concern |
| --- | --- |
| ctrlc 3.5.1 / rustc_driver_impl | Non-wasm dependency and handler call, but crate backend exports only Unix/Windows; add native handling or deliberately gate the optional handler. |
| memmap2 0.2.3 / rustc_data_structures::memmap | Non-Unix backend returns unsupported; use native mappings or the compiler's owned-byte-buffer path. Review `MmapMut::map_anon` length initialization when adapting the existing fallback. |
| stacker 0.1.21 / psm | Unconditional libc and mmap-based stack allocation need porting or a deliberate compiler stack strategy. |
| tempfile 3.23.0 | The `other` file backend returns unsupported; temporary file creation/persistence needs a native implementation. |
| getrandom 0.2.16 / 0.3.3 | Native backend selection needs review for the enabled dependency graph, rand default features (tempfile itself gates getrandom away on Scarlet). |
| jobserver 0.1.34 | A non-Unix fallback exists; verify its runtime behavior before relying on compiler parallelism. |

Disabling LLVM avoids its C++ runtime, but does not supply these other platform
interfaces. Do not make Scarlet globally `unix` to route around cfg failures.
