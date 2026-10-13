# Native C/C++ support

This module owns the native AArch64 C ABI facilities needed in addition to
the Rust `libscarlet_c.a` backend. It builds `libscarlet_cxx_support.a` and the
opt-in `libscarlet_c_startup.a`; applications use the same allocation, errno,
pthread, descriptor and stream implementation as Rust libc. The companion
does not supply another allocator, pthread implementation, standard stream
implementation, C++ standard library or operating-system ABI.

## Build and install

The SDK builds this source with its pinned LLVM libc source checkout. Numeric
conversion uses LLVM's shared header-only core; LLVM sources are not copied
into this module. `SCARLET_LLVM_LIBC_SOURCE_DIR` names the LLVM `libc` directory
containing `shared/str_to_float.h` and `src/__support/str_to_float.h`. The
validated dependency version is LLVM 21.1.8.

For a bootstrap build using raw Clang:

```sh
cmake -S user/lib/scarlet-libc/native -B build/native-c-support \
  -DCMAKE_TOOLCHAIN_FILE="$PWD/user/lib/scarlet-libc/native/cmake/aarch64-scarlet.cmake" \
  -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
  -DCMAKE_AR=llvm-ar -DCMAKE_RANLIB=llvm-ranlib \
  -DSCARLET_LLVM_LIBC_SOURCE_DIR="$LLVM_SOURCE/libc" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_LIBDIR=lib \
  -DCMAKE_INSTALL_PREFIX="$SCARLET_PREFIX"
cmake --build build/native-c-support
cmake --install build/native-c-support
```

The bootstrap toolchain selects AArch64 ELF, Scarlet C headers and Clang
builtin headers, without host C/C++ include directories or libraries. LLVM
support must come from the SDK's declared dependency revision. An installed
SDK toolchain may instead select the native target and include paths itself.
Build the Rust libc for `aarch64-unknown-scarlet` with the matching Scarlet
Rust toolchain and install `libscarlet_c.a` in the same prefix.

CMake consumers use `find_package(ScarletCXXSupport CONFIG REQUIRED)` and
`Scarlet::CXXSupport`. The package installs public Scarlet libc headers and
both archives. The final executable must also link the native Rust libc and
CRT, plus its actual C++ library dependencies. Archive groups may be needed
for a static engine's cyclic dependency closure.

## Startup and finalization

`Scarlet::CStartup` is for final executables whose native CRT does not yet
invoke the C completion hook. It adds `--wrap=main`, forces extraction of
`__wrap_main` and links `Scarlet::CXXSupport`. The adapter calls the actual
`main`, then `__scarlet_c_finish`, and returns the actual status to the native
CRT for TLS cleanup and termination. Shared engine libraries consume
`CXXSupport` without wrapping an entry point.

`__scarlet_c_finish` finalizes registered C/C++ callbacks and flushes streams.
A future CRT can invoke this hook directly before TLS cleanup and omit the
adapter. `exit` invokes the same completion function followed by native
`_Exit`. DSO finalization selects only matching registrations; a null DSO
selects all. Callbacks are removed before invocation, run in reverse
registration order, and cannot be called twice by recursive finalization.
There is no early `_Exit` path that skips successful finalization.

`SCARLET_LIBC_DIAGNOSTICS` enables per-callback address traces. It is optional
and does not replace or suppress any callback.

## Supported facilities

- C locale `strtof`, `strtod`, `strtold` and `atof` use LLVM conversion,
  preserve end pointers and report conversion errors through native errno.
- `setlocale` accepts queries and `C`/`POSIX`; other names fail with `ENOENT`.
  `localeconv` supplies the C locale. `perror` writes to the real stderr
  stream and preserves errno.
- `sscanf`/`vsscanf` parse byte strings, scansets, integers and float/double,
  with widths, suppression and accurate conversion counts. Wide text,
  long-double scanner directives, allocation, positional and unknown
  directives are rejected with `ENOTSUP` before assigning destinations.
  These are string scanners; no stream or wide scanner is supplied.
- AArch64 `setjmp`/`longjmp` preserve the AAPCS64 callee-saved integer
  registers, SP/LR and low 64 bits of v8–v15. `longjmp(..., 0)` returns 1.
  No signal-mask variant is supplied. x18 restoration is disabled by default;
  retained PAC variants require separate acceptance before use.
- `lround`/`lroundf`/`llround`/`llroundf` use ties away from zero independently
  of FPCR and do not introduce inexact on successful conversion. Invalid
  operands return the signed minimum and raise architectural invalid while
  preserving existing FPSR flags. `lrint` obeys FPCR and records inexact.
  `fegetround`/`fesetround` implement the AArch64 rounding-mode API only.
- `rand`/`srand` use a deterministic, atomic 32-bit LCG with a 31-bit result.
  Cryptographic callers must use the native entropy APIs.
- `mkdtemp`, `mkstemp`, `mkostemp` and `tmpfile` use actual native entropy and
  exclusive filesystem creation. `tmpfile` immediately unlinks its name and
  requires writable `/tmp`. `mkostemp` accepts `O_CLOEXEC` only.
- `strerror_r` uses the POSIX integer-result contract. `unlinkat` supports
  `AT_FDCWD` or absolute paths, and `AT_REMOVEDIR`; relative directory handles
  return `ENOTSUP`. Native `mprotect` belongs to Rust libc `mapping.rs`.

Floating-point printf remains outside this companion. The built-in formatter
continues to report unsupported floating conversions explicitly.

The public `uchar.h` supplies C `char16_t`/`char32_t` typedefs and uses C++'s
native keywords. It exposes the existing reserved `mbstate_t` storage layout
for header interoperability. Multibyte conversion functions are not supplied;
ICU consumers use ICU's implemented Unicode conversion APIs.

## Acceptance

`../tests/native-support.c` and `native-support-registers.S` exercise the
actual register, scanner, locale, stream and PRNG contracts. `math-round.c`
checks 176 named cases over 5632 calls with varied FPCR/FPSR state.
`native-startup.cpp` checks conversion end pointers/errors, recursive DSO
finalization, atexit order, and a real global destructor after `main` returns.
Compile these with builtins disabled, the native CRT, current Rust libc,
`CXXSupport` and `CStartup`; the C++ startup fixture uses the port's
`-fno-exceptions` profile. Running or compiling them on a host system does
not establish native acceptance. Preserve the exact ELF hash, child exit
status and complete named checks from real Scarlet guest runs.

The LLVM-derived setjmp sources retain their SPDX license and upstream
assembly. `source-provenance.json` records the source migration hashes;
individual guest results belong to validation artifacts, not build inputs.

The integrated archives passed all three fixtures on 2026-10-13 in real
AArch64/HVF guests with one and two CPUs: 68 native support checks, 176
rounding checks over 5632 calls, and 12 startup checks with exact destructor
order. Every child exited 0. These results do not replace acceptance of each
newly built SDK artifact or establish RV64 support for this companion.
