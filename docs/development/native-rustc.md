# Native rustc bring-up

This work targets `riscv64gc-unknown-scarlet` and `aarch64-unknown-scarlet` running
on Scarlet's native ABI. A Linux compiler running through Linux compatibility is
a separate milestone. Native rustc, its Cranelift backend, and the native Wild
linker have now completed the full guest acceptance path on both architectures.
The tools below retain the original audit and provide reproducible staging and
guest probes for subsequent toolchain builds.

## Actions build and artifact retrieval

Heavy native compiler builds run in the separate
[scarlet-rust-nix native-host pipeline](https://github.com/petitstrawberry/scarlet-rust-nix/actions/workflows/native-host.yml).
AArch64 and RV64 Cranelift builds are supported; the diagnostic dummy backend
remains selectable. The ordinary cached cross toolchain remains unchanged.
Successful native sysroots are downloadable by exact Actions run ID using that
repository's `scripts/fetch-native-host.sh`; the helper rejects dummy-backend
artifacts. Logs, source patch hashes, and bootstrap configuration are retained
even when the build fails.

The pipeline applies additional version-pinned native compiler/CRT/Cranelift
and dependency ports. An Actions build establishes ELF identity and cross
compilation; the separate guest acceptance below establishes execution in
Scarlet. See [the build recipe](../../tools/native-rustc/HOST-BUILD.md).

The separate [native Wild linker pipeline](https://github.com/petitstrawberry/scarlet-rust-nix/actions/workflows/native-linker.yml)
now builds on Actions for both targets. Its initial native guest acceptance
linked fresh object/archive inputs and executed both outputs on AArch64 and
RV64. A subsequent test also linked and executed captured Rust std inputs on
both architectures. See [linker evidence and usage](../../tools/native-linker/README.md).
This removes the previously unimplemented build-time linker as a bring-up task;
the full native-rustc acceptance now uses that Wild build.

The [versioned native-toolchain packaging workflow](https://github.com/petitstrawberry/scarlet-rust-nix/actions/workflows/native-toolchain-release.yml)
combines exact native-host and Wild run IDs without rebuilding either component.
[Run 35581141086](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35581141086)
produced downloadable AArch64 and RV64 `v0.1.0-rc.1` artifacts with publication
disabled. Their downloaded checksums, archive manifests, and payload equivalence
to the guest-tested candidates are recorded below. Creating a GitHub release is
a separate opt-in workflow input.

## Current guest-verified status, 2026-09-21

The tested distribution layout installs under
`/opt/scarlet/toolchains/rust/<version>`. It contains `rustc`, its private
`librustc_driver`, the Cranelift backend, static target libraries, and Wild.
`/system/bin/scarlet-ld` remains owned by the matching Scarlet image and is not
part of the toolchain archive.

The Scarlet full distribution composes
[`bundles/rust-toolchain`](../../bundles/rust-toolchain/bundle.toml), which
installs the published `v0.1.0-rc.1` archives and selects that version through
`/opt/scarlet/toolchains/rust/current`. The common
[`bundles/base`](../../bundles/base/bundle.toml) installs the matching runtime
interpreter at `/system/bin/scarlet-ld` for AArch64 and RISC-V64; its
architecture filter omits that ELF64-only layer for RV32. Interactive shells
include both `/system/bin` and the selected toolchain's `bin` directory in
`PATH`; Cargo is not included yet.

| Target | VM | Result | Total guest run | Native-host Actions run |
| --- | --- | --- | ---: | --- |
| `aarch64-unknown-scarlet` | HVF, 4 CPUs | `FULL_PASS` | 10.368 s | [35566457388](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35566457388) |
| `riscv64gc-unknown-scarlet` | TCG, 4 CPUs | `FULL_PASS` | 89.262 s | [35574074906](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35574074906) |

Both runs used native Wild from [Actions run 35573648820](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35573648820)
and Scarlet commit `d8ad9bb514f263013734661fdaa166e48c3dcdf9`. Each run checked
`rustc -Vv`, target cfg, frontend analysis, Cranelift code generation, native
linking, exact program stdout, and exit status 37. The
[versioned-bundle evidence](../../tools/native-rustc/evidence/2026-09-21-toolchain-bundle.json)
records component hashes and phase timings.

The RV64 compiler currently warns that target feature `d` must be enabled for
the target ABI. It does not fail this toolchain, but the target specification
must be corrected before a future rustc turns that warning into an error. Cargo,
procedural-macro execution, and compiler self-hosting remain unverified.

## Pre-bring-up baseline, 2026-09-21

The inspected Rust fork was
`petitstrawberry/rust@39c689a4859b9d8ee1828720135defd125c03d31`.
The cached toolchain's `manifest.toml` records that revision and host
`aarch64-apple-darwin`; `rustc -Vv` reports 1.94.0-nightly and LLVM 21.1.8.
The normal `scripts/scarlet-rust-dev.sh` activates a development-host compiler;
building `--target ...-scarlet` adds Scarlet output support, not a Scarlet host
compiler. The separate local scarlet-rust-nix checkout was on another revision,
so it was inspected without editing or using it to replace the locked toolchain.

Both 64-bit targets were probed with the cached toolchain:

| Probe | Result |
| --- | --- |
| Static ordinary `std` hello | Cross-built native ELF, OSABI `0x53` |
| Guest `native-rustc-probe` runner | Cross-built native ELF, OSABI `0x53` |
| `--crate-type=cdylib` and `dylib` | Unsupported; compiler exits **0** with a warning and produces no artifact |
| Upstream libloading 0.8.9 `Library` API | Fails with E0432; the export is gated out |
| Experimental libloading adapters, 0.8.9 and 0.9.0 | Rlib + compiler-like API metadata cross-compilation succeeds on both targets |
| ELF audit tests | Nine tests pass, including sectionless DSOs, TLS/versioning, and malformed input |
| Native compiler `-Vv`, cfg, frontend | Not run; no native compiler artifact exists |

The [recorded baseline evidence](../../tools/native-rustc/evidence/2026-09-21-baseline.json)
contains the final shipped-tool commands, every phase's stdout/stderr and status,
and ELF hashes for both targets. The absolute artifact paths in that file are
local test outputs; rerun the commands with fresh output directories elsewhere.

`probe_target.py` checks that requested artifacts actually exist, rather than
treating rustc's unsupported-crate-type warning as success. Logs include exact
arguments, stdout, stderr, compiler exit status, and separate pass/fail results.
It never downloads or alters a toolchain and refuses to overwrite prior output.

```sh
python3 tools/native-rustc/probe_target.py \
  --rustc "$SCARLET_RUST_TOOLCHAIN/bin/rustc" \
  --target riscv64gc-unknown-scarlet \
  --output /tmp/native-rustc-rv64 \
  --libloading-source /absolute/path/to/libloading-0.8.9
```

Repeat with `--target aarch64-unknown-scarlet` and a fresh output directory.
An exit status of 1 is expected on the baseline because dynamic crates and
libloading are blocked. The optional libloading check directly compiles the
unmodified 0.8.9 crate; future platform dependencies may require Cargo instead.

A separate [Rust DSO smoke builder](../../tools/loader-smoke/build-rust-dso.py)
subsequently built a real `no_std` Rust `cdylib` for both RV64 and AArch64 by
creating an isolated JSON target with PIC/dynamic linking and rebuilding
`core`/`compiler_builtins` with `-Zbuild-std`. This does not change the baseline
built-in targets above. It proves Rust-generated shared-object output for a C
entry point. Both architectures subsequently loaded and called that library in
a Scarlet QEMU guest, together with startup dependencies and runtime plugins;
see the [guest validation record](../../tools/loader-smoke/evidence/2026-09-21.json).
By itself this did not prove Rust `dylib`, shared std, or native rustc. For example:

```sh
python3 tools/loader-smoke/build-rust-dso.py \
  --arch aarch64 --toolchain "$SCARLET_RUST_TOOLCHAIN" \
  --output /tmp/scarlet-rust-dso-aarch64 --offline
```

## Initial source findings and prepared patches

The initial audit of the version-pinned source established these independent
compiler porting gaps:

1. The [RV64 target](https://github.com/petitstrawberry/rust/blob/39c689a4859b9d8ee1828720135defd125c03d31/compiler/rustc_target/src/spec/targets/riscv64gc_unknown_scarlet.rs)
   and [AArch64 target](https://github.com/petitstrawberry/rust/blob/39c689a4859b9d8ee1828720135defd125c03d31/compiler/rustc_target/src/spec/targets/aarch64_unknown_scarlet.rs)
   use static relocation, disable native ELF TLS, and do not enable dynamic
   linking. `rustc_driver` is a `dylib`. Built-in target validation also requires
   PIC when dynamic linking is enabled.
2. Scarlet's cfg has neither `unix` nor `windows`. Locked libloading 0.8.9 and
   0.9.0 export `Library`/`Symbol` only for those platforms. The direct E0432 probe
   confirms this is a compile blocker, independently of the kernel loader.
3. [std env constants](https://github.com/petitstrawberry/rust/blob/39c689a4859b9d8ee1828720135defd125c03d31/library/std/src/sys/env_consts.rs)
   have no Scarlet branch; the fallback DLL prefix/suffix and OS are empty.
   Backend filename discovery needs actual Scarlet values.
4. [std's Unix-style path implementation](https://github.com/petitstrawberry/rust/blob/39c689a4859b9d8ee1828720135defd125c03d31/library/std/src/sys/path/unix.rs#L65)
   omits Scarlet from `is_absolute`'s cfg list. Because Scarlet is not `unix`, it
   takes the prefix-requiring branch, and `/init` incorrectly tests as relative.
   This preexisting bug surfaced during the loader's QEMU validation. The Rust
   patch adds `target_os = "scarlet"` to that list; the loader uses `has_root()`
   as a bounded workaround with the existing installed std. The source patch
   passes `git apply --check` against the audited revision but has not been
   rebuilt into the installed toolchain.
5. Full rustc also needs its native dependency closure. The built-in LLVM backend
   requires a Scarlet-compatible LLVM/C++ runtime, allocation/threading/filesystem
   interfaces, and any enabled compression libraries. The loader alone does not
   supply those interfaces. Their complete native build is still unverified.

[The patch instructions](../../tools/native-rustc/patches/README.md) describe an
exact patch for the audited Rust revision and separate libloading adapters.
They leave `has_thread_local = false`, panic=abort, and Scarlet's empty target
family intact. `host_tools = true` in that patch is experimental metadata, not a
claim that the port is ready to ship. Apply them to an isolated Rust worktree,
rebuild the cross compiler and Scarlet std together, and then rerun target probes.
Existing static sysroot rlibs cannot be assumed suitable for PIC compiler DSOs.

The [bootstrap implementation](https://github.com/petitstrawberry/rust/blob/39c689a4859b9d8ee1828720135defd125c03d31/src/bootstrap/src/core/builder/mod.rs)
already statically links std into rustc_driver except on windows-gnu. A global
OS `libstd.so` is unnecessary. Every compiler, driver, backend, proc-macro, and
sysroot artifact must still come from one compatible compiler build.

`--host ...-scarlet`, rather than just `--target ...-scarlet`, is required when
bootstrapping the native compiler. Use an explicit experiment `--config` and
`--build-dir`; reuse neither the existing Rust build directory nor the installed
Nix/Cargo caches as a patch destination. The exact host LLVM/linker configuration
is not supplied as a working build recipe because that runtime port is incomplete.

## Backend isolation

The [compiler backend selection code](https://github.com/petitstrawberry/rust/blob/39c689a4859b9d8ee1828720135defd125c03d31/compiler/rustc_interface/src/util.rs)
supports a built-in LLVM backend when compiled with `feature="llvm"`, a built-in
dummy backend, and externally loaded backend dylibs. A separate LLVM backend
`.so` is not required in every compiler configuration.

`rustc -Vv` initializes a backend to print version information. `-Zno-codegen`
skips later code generation, not every earlier initialization step. The guest
probe's `--dummy` mode passes `-Zcodegen-backend=dummy` to all three phases to
isolate the frontend from default backend initialization.

A deeper source audit and a bootstrap dry-run confirmed an LLVM-free native
compiler build selection: keep the development-host backend as `["llvm"]`, and
set `[target.aarch64-unknown-scarlet] codegen-backends = ["dummy"]`. Although
bootstrap warns that dummy is an unknown custom backend, its normal compiler
assembly explicitly skips building custom backend crates. It sets
`CFG_DEFAULT_CODEGEN_BACKEND=dummy` and omits the native compiler's LLVM feature.
No `rustc_codegen_dummy` crate or bootstrap enum patch is required.

The [Actions host-build recipe](../../tools/native-rustc/HOST-BUILD.md) and
[config generator](../../tools/native-rustc/prepare_host_build.py) create the
portable stage2 command, using an existing development-host LLVM and a freshly
rebuilt stage1 cross compiler. Heavy builds run through scarlet-rust-nix Actions.
The generator itself only writes configuration and command metadata.
Use `x build`; the audited `x check` path enables LLVM regardless of the target
backend list. Empty backend lists remain invalid.

The dummy compiler remains useful as an initial execution diagnostic. The
accepted toolchain uses `--backend cranelift` and native Wild. A successful
config/dry-run alone still does not establish guest execution, and the dummy
backend cannot generate executable code.

## Audit and stage a native build

The dependency-free audit reads ELF64 program headers, dynamic tags and relocation
tables; section headers may be stripped. It reports interpreter, dependencies,
symbol names referenced by relocations, TLS, relocation kinds, hash tables,
versioning, RELR, text relocations, initialization hooks, and RPATH/RUNPATH.

```sh
python3 tools/native-rustc/audit_elf.py \
  --machine riscv64 --scarlet \
  --require-interpreter /system/bin/scarlet-ld \
  /absolute/native-sysroot/bin/rustc

python3 tools/native-rustc/audit_elf.py \
  /absolute/native-sysroot/lib/librustc_driver-actual-hash.so

python3 -m unittest discover -s tools/native-rustc -p 'test_*.py'
```

This inventory does not certify that an ELF is loadable. Compare it to the current
[Scarlet loader contract](../../user/lib/scarlet-dl/README.md). Initially only
RV64 NONE/64/JUMP_SLOT/RELATIVE and AArch64
NONE/ABS64/GLOB_DAT/JUMP_SLOT/RELATIVE relocations are supported. ELF TLS, symbol
versioning, RELR, IFUNC and other relocation forms are not implemented. Native
OS-level std TLS is a different mechanism and remains enabled through std.

To stage an actual native build into a **fresh** overlay:

```sh
python3 tools/native-rustc/stage.py \
  --sysroot /absolute/native-sysroot \
  --target riscv64gc-unknown-scarlet \
  --source-commit 0123456789abcdef0123456789abcdef01234567 \
  --loader /absolute/native-build/scarlet-ld \
  --probe /tmp/native-rustc-rv64/native-rustc-probe \
  --output /tmp/native-rustc-overlay
```

Replace the example source commit with the actual full revision. Staging checks
ELF machine/OSABI/interpreter and refuses a cached Linux or macOS rustc. It copies
the compiler to `/opt/native-rustc`, matching target rlibs/rmeta and backend DSOs,
and the interpreter/probe to `/system/bin`. The manifest records file hashes and
ELF inventories with `executed_on_scarlet: false`.

`stage.py` is the lower-level bring-up overlay. Release packaging uses the
versioned `/opt/scarlet/toolchains/rust/<version>` layout described above and
does not copy `scarlet-ld`; the Scarlet image supplies it from `/system/bin`.

Because the initial loader does not search RPATH/RUNPATH, runtime shared objects
are also copied to `/system/lib` under their existing filenames. Rust driver
hashes stay toolchain-specific. Review non-hashed dependencies for collisions
before merging the overlay into an image. The stager does not include Cargo,
rustdoc, arbitrary data files or a linker, resolve every undefined symbol, or
claim the inventory satisfies all current loader restrictions.

## Guest acceptance probes

The native runner accepts a fresh output directory and captures every command,
stdout, stderr, and exit status. Its default diagnostic mode requires:

1. `rustc -Vv`: exit 0 and exact `host: ...-scarlet`.
2. `rustc --print cfg --target ...-scarlet`: exit 0 and `target_os="scarlet"`.
3. `rustc -Zno-codegen hello.rs`: exit 0 for an ordinary std program.

These phases write `FRONTEND_PASS` and print `NATIVE_RUSTC FRONTEND PASS` only.
They never claim code generation or full compilation. `--dummy` is available
only in this diagnostic mode.

Full acceptance additionally requires `--full --linker /actual/native/linker`.
The runner creates the source inside Scarlet, compiles it to a native ELF using
that native linker, then executes the resulting program. Success requires the
exact stdout `SCARLET_NATIVE_RUSTC_HELLO_OK` and exit status 37. Only this path
writes `PASS` and prints `NATIVE_RUSTC FULL PASS`. `--backend` can select the
matching native Cranelift DSO; `--linker-flavor` handles a direct linker such as
`ld.lld`. The native Wild binary is supplied by the separate linker Actions
artifact. The compiler-only artifact does not include a native assembler.

[run-qemu.py](../../tools/native-rustc/run-qemu.py) boots an isolated ext2 root
containing the prepared toolchain overlay. It requires the standard static
Scarlet bootstrap init, fresh native probe, matching kernel/interpreter, and
actual native backend/linker paths. Its default mode is full acceptance;
`--frontend-only` explicitly selects diagnostics. Run `--help` for all required
artifact arguments. The host preserves the private root disk and extracts guest
commands, outputs, compiled ELF, and acceptance evidence after execution.
Relative links within the staging tree are allowed for versioned toolchain
packages; broken links and links escaping the staging root are rejected.

Compiler phases default to 900 seconds, with a 60-second generated-program
limit. The outer VM deadline also provides cleanup because Scarlet's current
std cannot kill a child. A VirtIO RNG supplies entropy for the native getrandom
backend; a pseudo-random fallback is not accepted. Full acceptance without
`--proc-macro` does not establish proc-macro support. Compiler parallelism and
self-hosted rebuilding still require separate tests.

### Procedural macros

Add `--proc-macro` to the full guest probe to build two independent native macro
DSOs and load both in one compiler process. The fixture exercises function-like,
attribute and derive macros, token parsing/iteration, and a macro-created thread
whose TLS destructor must run before join returns. The generated application
must print `SCARLET_NATIVE_PROC_MACRO_OK=42` and exit zero. This adds
`PROC_MACRO_PASS`, four phase logs, and `proc_macro_verified` to the evidence.
The ordinary std hello still has to pass first. A failed compiler returning 139
also records the kernel fault log when `/dev/kmsg` is available.

The [2026-09-22 AArch64 evidence](../../tools/native-rustc/evidence/2026-09-22-proc-macro-aarch64.json)
records a QEMU/HVF pass using the existing local native compiler and a rebuilt,
matching sysroot. The released `v0.1.0-rc.1` runtime fails macro expansion because
independent static std copies allocate the same TLS key numbers in a shared
thread table. The fix gives each std instance a namespace, shares the namespace
list through the thread pointer, and runs destructors across all namespaces.
std remains statically linked into applications and macro libraries.

The distribution rebuild is tracked in
[scarlet-rust-nix PR #25](https://github.com/petitstrawberry/scarlet-rust-nix/pull/25).
This local evidence does not validate the resulting CI artifacts or RV64, and
does not upgrade an installed bundle. All components must be rebuilt and shipped
together; Rust crate metadata from the local commit-stamped compiler cannot be
mixed with the existing unstamped Actions compiler. The native target still
uses `panic=abort`, so a panicking macro can terminate the compiler.

### Cargo port status

Cargo is not yet included or executable as a native Scarlet component. A build
attempt uses the Rust fork's pinned Cargo revision
`94c368ad2b9db0f0da5bdd8421cea13786ce4412` (Cargo 0.95.0). The initial stop is
`getrandom 0.2.16` lacking a native entropy backend. Applying native backends to
getrandom 0.2.16/0.3.4 and reusing the tempfile port advances the check to:

- `gix-sec 0.12.2`: file owner information and `libc::geteuid`.
- `filetime 0.2.26`: Unix file descriptors, metadata and timestamp operations.
- Native C dependencies: Cargo unconditionally depends on curl, libgit2 and
  bundled SQLite. `--no-default-features` does not remove those dependencies;
  a separate build attempt already fails in libz-sys without native C headers.

These need real Scarlet implementations or explicit upstream feature boundaries.
Treating the target as Unix does not supply the missing ABI. The first Cargo
acceptance scenario remains an offline workspace with a path dependency, a build
script and a proc-macro crate, followed by execution of the produced binary.
