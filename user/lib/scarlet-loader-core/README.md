# scarlet-loader-core

A dependency-free `no_std` + `alloc` ELF64 loader. `Platform` supplies files,
zero-filled mappings, checked memory access and page protection; the core owns
ELF validation, breadth-first dependency loading, eager relocation, symbol
interposition and constructor ordering. The same `LoaderContext` serves startup
and later `dlopen` calls.

Supported:

- Little-endian AArch64 and RISC-V64 `ET_DYN` images.
- Adoption of a kernel-mapped `ET_EXEC` or PIE main via unsafe `load_existing`.
- `DT_NEEDED`, SysV/GNU hash tables, weak/global/local/hidden/protected symbols.
- RELA relative, absolute, GLOB_DAT (AArch64) and JUMP_SLOT relocations, including
  PLT tables. RISC-V JUMP_SLOT correctly ignores its addend.
- Page permissions with no writable executable pages; GNU RELRO seals complete
  pages, matching the convention that linkers pad the RELRO end to a page boundary.
- Dependency-first `DT_INIT` / `DT_INIT_ARRAY` addresses, returned for the caller to
  execute once. The adopted main's startup owns its own initializers.
- New mappings roll back on loading, symbol resolution, constructor validation or
  protection failure. Every patch is resolved before relocation writes begin.
- A per-object export index avoids scanning all symbols for every relocation.

Explicitly unsupported: TLS, symbol versions, GNU IFUNC/UNIQUE, COPY and instruction
relocations, REL/RELR, executable stacks, text relocations, RPATH/RUNPATH,
DT_SYMBOLIC, preinit arrays and architecture-specific symbol visibility. Runtime
ABI functions can be registered with `add_symbol`. `lookup` searches a handle's
breadth-first dependency scope; `lookup_global` searches the complete context.

The scope is process-global. Unloading, lazy binding, destructor execution and
RTLD_LOCAL are not implemented. Keep the context alive while loaded code runs.
Mapping ownership belongs to the context, except for the adopted main. A failure
while writing or protecting an adopted main may leave it changed, so startup must
terminate on error. File and mapping operations are trusted platform contracts;
this parser is not an execution sandbox for hostile native code.

Resource bounds: 256 objects, 256 needed entries per object, 4096 dynamic entries,
1 GiB virtual image span, and one million symbols/relocations/hash-chain visits
per table. A `Platform` should apply its own file-read and address-space limits.

Run host checks without a target sysroot:

```sh
cargo +stable test --manifest-path user/lib/scarlet-loader-core/Cargo.toml
cargo +stable clippy --manifest-path user/lib/scarlet-loader-core/Cargo.toml --all-targets -- -D warnings
```

The `check` example reads the real compiler/linker-produced smoke fixtures,
performs all relocations in simulated process memory, validates permissions and
constructor addresses, and loads the runtime plugin. It never executes target
machine code:

```sh
python3 tools/loader-smoke/build-fixtures.py --arch aarch64 --output target/loader-smoke/aarch64
cargo +stable run --manifest-path user/lib/scarlet-loader-core/Cargo.toml \
  --example check -- target/loader-smoke/aarch64/staging aarch64
```
