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
must be corrected before a future rustc turns that warning into an error. Cargo
and compiler self-hosting remain unverified. The later AArch64 proc-macro and
filesystem acceptance below uses an updated local toolchain.

## Native Cargo HTTPS acceptance, 2026-09-23

A local AArch64 HVF guest now runs a cross-built Scarlet-native Cargo. With an
empty Cargo home it fetches `itoa 1.0.15` from crates.io over HTTPS, compiles
the dependency and an application with the native compiler, and runs the
application. A negative control substitutes an unrelated root CA and reaches
the TLS handshake but fails with `UnknownIssuer`. The CA source, client
selection, update path, and trust limits are described in
[Native Cargo TLS trust](native-cargo-trust.md). The
[guest result and diagnostic logs](../../tools/native-rustc/evidence/2026-09-23-cargo-online-aarch64/result.json)
record the positive run and the untrusted-CA control.

This acceptance is local and does not change the published RC toolchain. Its
online build target is guest tmpfs because ext2 still rejects removal of an
empty rustc temporary archive directory. A default disk-backed Cargo workflow
and Cargo/proc-macro integration in the distributed toolchain remain separate
acceptance steps. The Cargo port uses exact Git revisions of Scarlet-specific
crate forks; remaining local dependency ports are still being migrated.

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

### Initial Cargo port assessment, 2026-09-22 (superseded)

This section records the original dependency failures before the native Cargo
HTTPS acceptance above. Its status statements describe that earlier build.

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

The filesystem work below now supplies real timestamp operations and error
reporting in Rust std. A `filetime` Scarlet backend still needs to select those
APIs; merely rebuilding the unchanged crate does not make it portable.

The remaining dependencies need real Scarlet implementations or explicit upstream feature boundaries.
Treating the target as Unix does not supply the missing ABI. The first Cargo
acceptance scenario remains an offline workspace with a path dependency, a build
script and a proc-macro crate, followed by execution of the produced binary.

### Native filesystem and initial libc, 2026-09-22

The [AArch64 filesystem evidence](../../tools/native-rustc/evidence/2026-09-22-native-fs-aarch64.json)
records a release-kernel HVF run with four CPUs. The guest returned `FULL_PASS`
with both `native_fs_verified` and `proc_macro_verified`. Tests cover:

- Rust `canonicalize` and C `realpath`, including symlinks before `..`, empty
  and missing paths, non-directory components, trailing slashes and link loops.
- The 40-link total lookup limit, exact error numbers, and undersized output
  buffers remaining unchanged.
- Mount-aware canonical paths and cwd on ext2 and a private tmpfs mount below
  `/native-rustc-output`, preserving the mountpoint's parent directories.
- C allocation/reallocation, descriptor-relative `utimensat`, nofollow link
  times, `futimens`, `fsync`, and Rust `FileTimes` after renaming an open file.
- ext2 range checking without a partial timestamp change, preservation through
  writeback, and independent extraction of the final inode from the disk image.
- The existing native std program and two proc-macro DSOs, including execution
  of their generated program.

The native ABI adds six filesystem operations, documented in
[Native filesystem extensions](../abi/native-filesystem.md). Legacy calls keep
their existing error contract. The updated std uses detailed metadata/mkdir
errors, so `create_dir_all` can recognize missing parents. It no longer treats
canonicalization as string concatenation or `sync_all` as a no-op.

[`scarlet-libc`](../../user/lib/scarlet-libc/README.md) is the initial static
C ABI adapter, with headers and an actual C fixture linked into the guest probe.
It is not yet a complete C library for Cargo's curl, libgit2 and SQLite dependencies.
Scarlet remains outside `cfg(unix)` and retains its Native syscall numbering.

Build the enhanced probe with a matching Scarlet cross compiler and updated
sysroot, selecting a Clang that supports the requested architecture:

```sh
RUSTC=/path/to/matching-cross-rustc \
SCARLET_PROBE_CC=/path/to/clang SCARLET_PROBE_AR=/path/to/llvm-ar \
cargo build --manifest-path tools/native-rustc/Cargo.toml \
  --release --target aarch64-unknown-scarlet --features native-fs
```

Pass that binary as `--probe` and add `--native-fs --proc-macro` to the full
`run-qemu.py` command above. RV64 uses `riscv64gc-unknown-scarlet`; the C fixture
explicitly matches its RV64GC/lp64d ABI. Both target std libraries and both C/Rust
probe binaries cross-built locally. This filesystem change has guest execution
evidence for AArch64 only.

The tested sysroot is a local experimental overlay. The compiler/driver were
reused from the compatible previous local build; the probe and newly compiled
applications use the rebuilt std. This does not update the published
`v0.1.0-rc.1` archive or the bundle pin. Adopting the overlay requires both the
new kernel syscalls and a matching rebuilt toolchain; full compiler rebuilds run
in [scarlet-rust-nix PR #26](https://github.com/petitstrawberry/scarlet-rust-nix/pull/26)
on Actions ([AArch64](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35690454081),
[RV64](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35690489167)).

### libc and filesystem hardening, 2026-09-22

The [follow-up evidence](../../tools/native-rustc/evidence/2026-09-22-libc-quality-aarch64.json)
records the updated AArch64 release kernel and probe on HVF, plus a complete
1273-test release kernel run. The changes address:

- Odd-sized allocations leaving the next std allocator block header unaligned.
  The toolchain overlay rounds split points and tests the actual placement
  helpers before bootstrap. Native C checks stress fragmentation and realloc.
- LLVM builtin recognition deleting an overflowing Rust call to the exported
  `calloc`. `scarlet-libc` now uses `#![no_builtins]`; the optimized guest checks
  both the null return and child-thread `ENOMEM`, preserving the parent's errno.
  A [minimal codegen reproduction](../../tools/native-rustc/evidence/2026-09-22-libc-quality-aarch64/calloc-codegen/build.py)
  and before/after IR preserve the failure independently of guest execution.
- ext2 fsync through a second descriptor missing another descriptor's writes.
  Shared inode locking now covers writes, size publication, timestamps, truncate
  and writeback. Tests check the former publication race and read raw disk bytes.
- Explicit mtime overwriting ctime during writeback, and directory timestamp
  updates freezing cached directory size.
- Renames replacing entry objects and leaving cwd, open directory handles and
  descendant mounts attached to old paths. Renames retain and relocate entries;
  tests cover cycles, mount roots and symlink/trailing-slash cases. Directory
  renames to new names ending in `/` follow Linux behavior.

The 44 header checks cover standalone/repeated inclusion and aggregate C11/C++11
declarations on AArch64/RV64 without host libc headers. Both native C/Rust probes
cross-build; filesystem guest execution remains AArch64-only. The published
bundle is unchanged. Allocator rebuilds are tracked in
[toolchain PR #27](https://github.com/petitstrawberry/scarlet-rust-nix/pull/27)
([AArch64](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35693141878),
[RV64](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35693166722)).

The chosen direction is a Rust libc backed by Scarlet Rust std. Its
[acceptance gates](../../user/lib/scarlet-libc/STATUS.md) describe the work still
required for C startup with backend initialization, allocation-free errno,
standards coverage and real Cargo dependencies. Keeping std as the backend is
supported; `no_std` is not a prerequisite for these compatibility and quality
goals.


### std-backed libc errno and C startup, 2026-09-22

The [errno milestone evidence](../../tools/native-rustc/evidence/2026-09-22-libc-errno-aarch64.json)
records a fresh AArch64/HVF `FULL_PASS` with four CPUs and the release kernel.
This run rebuilds std, loader, native rustc/driver/Cranelift and the guest probe
for the [versioned native TLS header](../abi/native-tls.md). The implementation
continues to use Rust std as its backend.

- `errno` occupies preallocated per-thread storage, initialized before
  constructors and child-thread code. First access does not allocate. A
  deterministic allocator returning null tests a pre-main constructor, a fresh
  thread, TLS destruction, `malloc`/`calloc` and preservation on failed realloc.
- `aligned_alloc`, `posix_memalign` and `reallocarray` extend the C allocation
  surface. C and Rust tests cover alignment, overflow and errno/output/old-buffer
  preservation according to each API's contract.
- An ordinary C constructor and `main` are compiled without host libc headers,
  linked directly with `scarlet-crt0.o` and the std-backed `libscarlet_c.a`, then
  executed on Scarlet with the required exit code 43. The reproducible builder
  and invocation are in the [libc README](../../user/lib/scarlet-libc/README.md).
  Pass its binary to the harness using `--c-startup-probe`; success requires
  both the persisted marker and matching exit status.
- Existing native filesystem, std hello compilation/execution, and two proc
  macro DSOs plus their consumer pass with the rebuilt runtime.

The harness also rejects conflicting same-named std, driver and backend DSOs
in the staged sysroot. A development run initially loaded an older driver from
`bin` ahead of the rebuilt copy in `lib`, causing exit 139 on the new TLS header.
The final run uses matching copies; the failure log and final inputs are retained
in the evidence. Always rebuild and stage the complete matching runtime set.

Both targets cross-build std, loader, libc, the enhanced probe and the direct-C
fixture. RV64 guest execution and installed SDK acceptance remain outstanding
for this milestone. Allocation fault injection does not establish physical OOM
recovery or async-signal safety. The published bundle is unchanged. Full compiler
builds for both targets are tracked in
[toolchain PR #28](https://github.com/petitstrawberry/scarlet-rust-nix/pull/28),
stacked on the allocator correction in #27.

### C library surface and upstream zlib, 2026-09-22

This milestone extends the std-backed libc with byte strings, C-locale
classification and integer conversion, native descriptor I/O, unbuffered
streams and integer/string formatted output. The
[support matrix](../../user/lib/scarlet-libc/STATUS.md) lists the exact surface
and remaining gaps. Stdio uses locked opaque `FILE` objects and one-byte
pushback. Formatted output supports C varargs and integer length modifiers;
float, wide, positional, grouping and `%n` formats return `ENOTSUP`. There is no
scanf family or stream-buffering interface. Integer conversion follows
C17/POSIX prefixes, without C23 `0b`/`0B`. The matching compiler-builtins provides
the five `memcpy`/`memmove`/`memset`/`memcmp`/`strlen` symbols.

The [Native descriptor extensions](../abi/native-descriptors.md) supply
status-preserving open, close, read/write, seek, duplicate and flag operations.
The kernel checks descriptor access mode, shares offset and append state
across duplicates, and keeps close-on-exec descriptor-local. Tmpfs and ext2
append select EOF and publish the write while holding the inode lock; this is
not implemented by a userspace seek followed by a write. These additive
operations leave old Native syscall error conventions intact. They do not
implement Unix credentials, umask or full permission enforcement.

The probe's `native-fs` feature now links C fixtures for
[strings/conversion](../../user/lib/scarlet-libc/tests/strings.c),
[descriptors](../../user/lib/scarlet-libc/tests/descriptor.c), and
[stdio](../../user/lib/scarlet-libc/tests/stdio.c), alongside the previous
filesystem/allocation checks. They are compiled with builtins disabled to call
the linked C ABI. The varargs fixture crosses register-save and stack argument
areas, so host Rust tests alone are insufficient evidence for either guest ABI.

The [zlib consumer builder](../../tools/native-rustc/consumer-zlib/README.md)
compiles all 15 unmodified core/gzip sources from the pinned upstream zlib 1.3.2
release, then statically links an ordinary C consumer with the matching CRT
and `libscarlet_c.a`. AArch64 and RV64 cross-build and ELF audits are complete.
The consumer tests one-shot and incremental compression, gzip file I/O and
seeking, integer/string `gzprintf`, duplicated-descriptor ownership, missing
files and corrupted CRC errors. Neither building the ELF nor auditing it
establishes guest execution.

Build the libc archive and enhanced probe as described above, then build the
consumer with a fresh output directory:

```sh
python3 tools/native-rustc/consumer-zlib/build.py \
  --target aarch64-unknown-scarlet \
  --sysroot /path/to/matching-cross-sysroot \
  --libc /path/to/aarch64-unknown-scarlet/release/libscarlet_c.a \
  --clang /path/to/unwrapped/clang --ar /path/to/llvm-ar \
  --linker /path/to/ld.lld \
  --output /tmp/scarlet-zlib-aarch64
```

The builder downloads the SHA-256-pinned archive by default; use
`--source-archive /path/to/zlib-1.3.2.tar.gz` to reuse a verified local copy.
It records tool versions, commands, input hashes, archive members and final ELF
properties, and uses only Scarlet and Clang builtin headers. For RV64, select
`riscv64gc-unknown-scarlet` and matching inputs.

Use a release kernel containing the descriptor changes, a complete matching
staged native toolchain, and the freshly rebuilt probe. For example, on an
Apple Silicon host with the versioned toolchain layout:

```sh
python3 tools/native-rustc/run-qemu.py \
  --arch aarch64 --accel hvf --cpus 4 --storage ext2 \
  --kernel /path/to/aarch64-release-scarlet \
  --staging /path/to/matching-staging-tree \
  --bootstrap /path/to/matching-probe-bootstrap \
  --probe /path/to/aarch64-release-native-rustc-probe \
  --rustc /opt/scarlet/toolchains/rust/v0.1.0-rc.1/bin/rustc \
  --sysroot /opt/scarlet/toolchains/rust/v0.1.0-rc.1 \
  --linker /opt/scarlet/toolchains/rust/v0.1.0-rc.1/bin/wild \
  --linker-flavor ld.lld --native-fs --proc-macro \
  --c-startup-probe /path/to/matching-c-startup-probe \
  --zlib-probe /tmp/scarlet-zlib-aarch64/zlib-probe \
  --output /tmp/scarlet-libc-zlib-aarch64
```

All host paths are examples; the output directory must be new. The versioned
guest path is a staging layout, not evidence that the published archive contains
these changes. `--zlib-probe` stages the static consumer and gives it a writable
guest output directory. Acceptance requires persisted `ZLIB_PASS`,
`zlib.status` with exit 47 and the exact `SCARLET_LIBC_ZLIB_OK` stdout line.
Only then does the harness set `zlib_verified`. The native std application,
proc-macro checks and optional direct-C startup check retain their own gates.

The [recorded AArch64/HVF run](../../tools/native-rustc/evidence/2026-09-22-libc-zlib-aarch64.json)
returns `FULL_PASS` with all five verification flags true: native compilation,
proc macros, native filesystem, direct-C startup and zlib. The C string,
descriptor and stdio fixtures pass on ext2 and tmpfs; zlib returns 47 and prints
the required success marker. A separate complete release kernel run passes all
1293 tests, including 20 added regressions. It also verifies the correction for
seeking with a detached ext2 node, found during this milestone's test run.
Host validation passes 33 libc tests, 27 ABI tests, three probe tests, 21 Python
tests and 56 header checks.

RV64 has cross-build and ELF-audit evidence, with guest execution outstanding
for this milestone. The published bundle remains unchanged. Complete standards
coverage, permissions and process/thread/network contracts, installed SDK
acceptance, and Cargo's curl/libgit2/SQLite dependencies remain separate work.


### Positioned C I/O and native SQLite, 2026-09-22

The next libc milestone adds `pread`/`pwrite`, offset-preserving `ftruncate`,
nonblocking whole-file `flock`, `unlink`/`rmdir` and `getcwd`. The
[Native descriptor contract](../abi/native-descriptors.md) documents syscall
numbers, errors and limits. Descriptor reservation now precedes create/truncate
side effects. Ext2 truncation is bounded to 16 MiB and last-link unlink of an
open ext2 file returns `EBUSY`. Ext2 rmdir returns `ENOTEMPTY` for nonempty
directories and `EOPNOTSUPP` before mutation for empty directories, because
retained-cwd lifetime is unresolved. These limits must not be presented as full
Unix file lifetime or unrestricted sparse-file support.

The C library also adds sorting/searching, span/tokenizer/case-folding routines,
clock/sleep and entropy APIs, `fabs`/`fabsf`, and `assert`/`abort`. Registered
entropy is required; Native failure maps to `EIO`, and recognized nonzero random
flags return `ENOTSUP`. Abort uses process exit 134 rather than `SIGABRT`.
[Runtime](../../user/lib/scarlet-libc/tests/runtime.c),
[path](../../user/lib/scarlet-libc/tests/path.c),
[positioned-I/O](../../user/lib/scarlet-libc/tests/positioned.c) and
[algorithm](../../user/lib/scarlet-libc/tests/algorithms.c) fixtures are linked
into the `native-fs` probe. Separate child processes exercise assertion and
abort termination.

The [SQLite builder](../../tools/native-rustc/consumer-sqlite/build.py) pins
SQLite 3.53.4 by SHA-256, SHA3-256 and source identity. It compiles the unmodified
amalgamation with Scarlet/Clang builtin headers and a separate
[Native VFS](../../tools/native-rustc/consumer-sqlite/scarlet_vfs.c), using
`SQLITE_OS_OTHER=1`. This is a SQLite platform port, not Unix-VFS or Cargo
acceptance. The build disables pthreads, WAL, mmap, extension loading and
localtime conversion, and uses memory temp storage. Rollback-journal operations
use positioned I/O and real exclusive nonblocking inode locks at every logical
SQLite lock level, deliberately serializing concurrent readers.

```sh
python3 tools/native-rustc/consumer-sqlite/build.py \
  --target aarch64-unknown-scarlet \
  --sysroot /path/to/matching-cross-sysroot \
  --libc /path/to/aarch64-unknown-scarlet/release/libscarlet_c.a \
  --clang /path/to/unwrapped/clang --ar /path/to/llvm-ar \
  --linker /path/to/ld.lld \
  --output /tmp/scarlet-sqlite-aarch64
```

Use `--source-archive /path/to/sqlite-amalgamation-3530400.zip` to reuse a pinned
local archive. Rebuild the release kernel, libc and native probe, then add
`--sqlite-probe /tmp/scarlet-sqlite-aarch64/sqlite-probe` to the complete HVF
command in the preceding section. The harness runs create, verify, forced
transaction exit and recovery as separate processes on both ext2 and tmpfs.
The forced-exit phase must first verify that a transaction change spilled into
the database and leave a hot journal, then exit 134 with its readiness marker.
The other phases must exit 53 and print `SCARLET_LIBC_SQLITE_OK`. The harness
snapshots both hot journals before recovery and checks every exit and marker.
It extracts the ext2 database after VM shutdown for independent host SQLite
integrity and exact-content checks. This tests process-exit recovery; power loss
is not simulated.

The [recorded AArch64/HVF run](../../tools/native-rustc/evidence/2026-09-22-libc-sqlite-aarch64.json)
returns `FULL_PASS` in 19.327 seconds, with all seven verification flags true.
The [saved logs and reports](../../tools/native-rustc/evidence/2026-09-22-libc-sqlite-aarch64/)
include all new C fixtures on ext2/tmpfs, assertion/abort child exits 134, and all
eight SQLite processes. The forced-exit phases verify actual dirty database
spill and leave 405504-byte hot journals before recovery. Independent host
SQLite validates the recovered ext2 database's integrity, foreign keys, all
24 rows and the complete 131113-byte BLOB. Plain C startup, zlib, native Rust
compilation/execution and proc macros also remain passing.

The complete unfiltered release kernel suite passes 1328 tests, including
35 added regressions. The guest exposed tmpfs unlink prematurely erasing an
open file's cached data; final-node ownership now retains data/cache/quota, and
pinned pages retire only after unpin. Three dedicated kernel tests cover that
fix. Host checks pass 47 libc tests, 27 ABI tests, 80 header checks, 31 harness
tests and four probe tests; repository formatting passes. Final RV64/RV32
kernel compilation passes, and RV64 libc/probe/SQLite cross-build and ELF audits
pass. RV64 guest execution, physical OOM, power-loss durability, published bundle
and general Cargo/installed SDK acceptance remain outstanding.
