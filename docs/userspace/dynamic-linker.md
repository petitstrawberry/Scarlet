# Scarlet native dynamic linking

Scarlet's native dynamic linker consists of three components:

- `user/lib/scarlet-loader-core`: allocation-backed ELF parsing, dependency
  loading, symbol resolution and relocation, independent of kernel syscalls.
- `user/lib/scarlet-dl`: native file/memory operations and the process loader
  context, including the C dynamic-loading entry points.
- `user/scarlet-ld`: a statically linked interpreter at
  `/bin/scarlet-ld`, selected by an executable's `PT_INTERP`.

Base retains `/system/bin/scarlet-ld` as a compatibility symlink for native
Rust binaries published before the interpreter moved to `/bin`.

The loader is an initial implementation for 64-bit little-endian AArch64 and
RISC-V ELF. It is not a Linux compatibility layer. Ordinary native applications
continue to use their existing static runtime. Rust shared objects must be
built using a matching compiler and dependency set; the loader does not make
Rust's ABI stable.

## Kernel contract

The kernel maps the executable and interpreter. On interpreter entry, the stack
is 16-byte aligned and contains `argc`, the terminated `argv` array, terminated
`envp`, and auxiliary-vector pairs ending at `AT_NULL`. AArch64 `x0/x1` and
RISC-V `a0/a1` also contain `argc/argv`. `AT_PHDR`, `AT_PHENT` and `AT_PHNUM`
describe the main executable; `AT_ENTRY` is its relocated entry; `AT_BASE` is
the interpreter load bias. Native dynamic executables receive `AT_EXECFD`,
a native file handle for the actual executable, independent of caller-controlled
`argv[0]` and filesystem-view changes during exec. The interpreter consumes and
closes this handle after reading the main image.

The interpreter records the original stack pointer before starting its own Rust
runtime. It adopts the existing main mapping, loads and relocates dependencies,
then returns control to the original executable entry with that stack and the
native argument registers restored. It remains mapped for the lifetime of the
process, owning the same object namespace used by subsequent dynamic loads.

Native syscall `MemoryProtect` (702) takes address, byte length and protection
bits (`read=1`, `write=2`, `execute=4`). Address must be page-aligned; length must
be nonzero and is rounded up. It returns zero on success or `usize::MAX` on
failure. The entire range must be user mapped. Private anonymous/task-backed
memory can gain permissions; object/device mappings can only reduce them.
Changes preserve backing and invalidate old page mappings; subsequent executable
page installation performs the architecture's instruction-cache synchronization.

## Initial scope

The loader resolves `DT_NEEDED` dependencies eagerly, handles SysV and GNU
symbol hashes, and applies RELA relocations. AArch64 supports `ABS64`,
`GLOB_DAT`, `JUMP_SLOT` and `RELATIVE`; RISC-V64 supports `64`, `JUMP_SLOT` and
`RELATIVE`. Unresolved weak references become zero. Relocations are applied
before final segment permissions and GNU RELRO protection. Dependency
constructors run before their consumers. The executable's own startup remains
responsible for its main initialization array.

The interpreter supplies `dlopen`, `dlsym`, `dlclose` and `dlerror` symbols.
The initial API requires `RTLD_NOW | RTLD_GLOBAL` (`0x102`); it does not provide
local scopes or lazy binding. Constructor reentry and concurrent loader calls
return an explicit busy error. Closing a handle does not unmap its code: loaded
objects stay pinned until process exit. RPATH/RUNPATH, TLS, versioned symbols, IFUNC, COPY
relocations and RELR are outside this initial scope and must not be assumed to
work. This is insufficient by itself to run an unmodified native rustc.

See [the executable smoke test](../../tools/loader-smoke/README.md) for building
fixtures and booting an isolated guest, and
[native rustc bring-up](../development/native-rustc.md) for the compiler target
and dependency changes still required. Test success only establishes the
behaviors exercised by the fixtures, not arbitrary ELF compatibility.

## Verified guest execution, 2026-09-21

AArch64 and RISC-V64 QEMU guests passed startup linking, dependency constructors,
main/plugin symbol sharing, `dlopen`/`dlsym`/`dlclose`/`dlerror`, and a real Rust
`no_std` `cdylib` function call returning 42. Both architectures also passed with
GNU-only hashes in the C fixtures. The Rust fixture exercises data/GOT
relocations; it uses a C entry point and does not establish Rust `dylib` ABI
compatibility or compiler execution.

The [validation record](../../tools/loader-smoke/evidence/2026-09-21.json)
contains the QEMU commands, success markers, artifact hashes, and successful
AArch64 (1,266) / RISC-V64 (1,293) kernel regression counts. Reproduction commands
are in the smoke-test README linked above. Native rustc remains unbuilt and
unexecuted.
