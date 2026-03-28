# Loadable Scarlet Module (LSM)

Runtime kernel module loader for Scarlet OS. Loads `.lsm` (relocatable ELF) files into kernel space, resolves symbols, applies relocations, and calls module init functions.

## Overview

- `.lsm` = ELF64 relocatable object (`ET_REL`), self-implemented parser (no external crates)
- Modules are `#![no_std]` Rust crates depending on `scarlet` kernel crate
- Dependency resolution in userspace (`lsm_load`), kernel only validates and registers
- Supported architectures: RISC-V 64, AArch64

## Quick Start

```sh
cargo make build-modules-debug-riscv64
cargo make build-initramfs-debug-riscv64
cargo make run-debug-riscv64
```

```
# lsm_load lsm-test
loading module: /scarlet/system/scarlet/modules/lsm-test.lsm
[lsm-test] Loadable Scarlet Module loaded successfully!
module loaded successfully

# lsm_list
1 module(s) loaded:
ID    NAME
1     lsm-test

# lsm_unload lsm-test
module 'lsm-test' (id=1) unloaded successfully
```

## Module Anatomy

### Directory Structure

```
modules/loadable/lsm-test/
  Cargo.toml          # crate metadata, scarlet dependency
  module.toml         # module name and dependencies
  build.rs            # reads module.toml, sets RUSTC_VERSION/TARGET/SCARLET_LSM_DEPENDS
  src/lib.rs          # module code with required symbols
  .cargo/config.toml  # build-std for no_std
```

### module.toml

```toml
[module]
name = "lsm-test"
depends = []
```

`name` is the module identity used by dependency resolution, `lsm_list`, and `lsm_unload`. The `.lsm` filename is derived from this name.

### Required Symbols

Every module must export these `#[unsafe(no_mangle)]` symbols:

| Symbol | Type | Purpose |
|---|---|---|
| `SCARLET_LSM_NAME` | `[u8; N]` | Module name, null-terminated |
| `SCARLET_LSM_BUILD_INFO` | `[u8; N]` | `rustc --version;target`, validated against kernel |
| `SCARLET_LSM_DEPENDS` | `[u8; N]` | Comma-separated dependency list, populated by `build.rs` |
| `scarlet_lsm_init` | `extern "C" fn() -> Result<(), &'static str>` | Entry point, called after relocation |

### Example lib.rs

```rust
#![no_std]

use scarlet::early_println;

#[unsafe(no_mangle)]
pub static SCARLET_LSM_NAME: [u8; 9] = *b"lsm-test\0";

#[unsafe(no_mangle)]
pub static SCARLET_LSM_BUILD_INFO: [u8; 72] = {
    let s = concat!(env!("RUSTC_VERSION"), ";", env!("TARGET"), "\0");
    let bytes: &[u8] = s.as_bytes();
    let mut arr = [0u8; 72];
    let mut i = 0;
    while i < bytes.len() && i < 72 {
        arr[i] = bytes[i];
        i += 1;
    }
    arr
};

#[unsafe(no_mangle)]
pub static SCARLET_LSM_DEPENDS: [u8; 256] = {
    let s = concat!(env!("SCARLET_LSM_DEPENDS"), "\0");
    let bytes: &[u8] = s.as_bytes();
    let mut arr = [0u8; 256];
    let mut i = 0;
    while i < bytes.len() && i < 256 {
        arr[i] = bytes[i];
        i += 1;
    }
    arr
};

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_lsm_init() -> Result<(), &'static str> {
    early_println!("[lsm-test] loaded!");
    Ok(())
}
```

### Cargo.toml

```toml
[package]
name = "scarlet-module-lsm-test"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
scarlet = { path = "../../../kernel" }
```

### .cargo/config.toml

```toml
[target.riscv64gc-unknown-none-elf]
runner = "true"

[target.aarch64-unknown-none-elf]
runner = "true"

[profile.dev]
opt-level = 3

[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]
unstable-options = true
```

### build.rs

`build.rs` reads `module.toml`, runs `rustc --version`, and sets env vars for the `SCARLET_LSM_*` symbols. `SCARLET_LSM_DEPENDS` is populated from `module.toml`'s `depends` array.

## Build Pipeline

### cargo-scarlet

```sh
cargo scarlet build --module modules/loadable/lsm-test \
  --target kernel/targets/riscv64gc-unknown-none-elf.json \
  --output mkfs/dist/modules/riscv64gc-unknown-none-elf
```

What happens:
1. `cargo rustc --target <json> --emit=obj` compiles the module
2. The `.o` file is renamed to `<module-name>.lsm` (name from `module.toml`)
3. The `.lsm` is copied to the output directory

### Makefile.toml Integration

Module build tasks run cargo-scarlet for each module in dependency order:

| Task | Modules | Profile |
|---|---|---|
| `build-modules-debug-riscv64` | lsm-test, lsm-dep-test | debug |
| `build-modules-debug-aarch64` | lsm-test, lsm-dep-test | debug |
| `build-modules-release-riscv64` | lsm-test, lsm-dep-test | release |
| `build-modules-release-aarch64` | lsm-test, lsm-dep-test | release |

Output: `mkfs/dist/modules/<triple>/<name>.lsm`

### Initramfs

`mkfs/make_initramfs.sh` copies all `*.lsm` from `dist/modules/<triple>/` to `initramfs/system/scarlet/modules/`.

## Userspace Tools

### lsm_load

```
lsm_load <path_or_name>
```

Path resolution order:
1. Absolute path (`/scarlet/system/scarlet/modules/lsm-test.lsm`) — used as-is
2. Relative path that exists on filesystem (`modules/lsm-test.lsm`) — resolved to absolute
3. Module name without extension (`lsm-test`) — searched in module directories with `.lsm` appended

Module directories (searched in order):
- `$LSM_MODULES_PATH` directories (colon-separated, e.g. `/scarlet/modules:/extra/modules`)
- Default: `/scarlet/system/scarlet/modules`

Dependency resolution:
- Parses `SCARLET_LSM_DEPENDS` from module ELF
- Recursively loads dependencies before the target module
- Cycle detection prevents circular dependencies
- Skips already-loaded modules

### lsm_unload

```
lsm_unload <module_name>
```

Resolves module name to ID via `lsm_list`, then calls the unload syscall. Refuses if other loaded modules depend on the target.

### lsm_list

```
lsm_list
```

Lists all loaded modules with ID and name.

## Kernel Loading Process

1. **ELF parsing** — Validates ELF64 magic, class, endianness, `ET_REL` type. Only `SHT_RELA` relocations supported (`SHT_REL` rejected).
2. **Section mapping** — Each `SHF_ALLOC` section is page-aligned and mapped into the module VM region. All sections get write permission for relocation.
3. **Content copy** — `SHT_PROGBITS` sections are copied, `SHT_NOBITS` sections are zero-filled.
4. **Build info validation** — `SCARLET_LSM_BUILD_INFO` is compared against the kernel's `RUSTC_VERSION;TARGET`. Mismatch returns error 7.
5. **Dependency validation** — All declared dependencies must be present in `MODULE_REGISTRY`. Missing dependency returns error 10.
6. **Relocation** — Architecture-specific relocation handler patches all references. Symbols resolved against the global `SymbolRegistry`.
7. **icache flush** — Architecture-specific cache maintenance (RISC-V: `fence.i`, AArch64: DC CVAU + IC IALLU + DSB + ISB).
8. **Permission finalization** — `.text` sections (with `SHF_EXECINSTR`) are remapped to Read+Execute (Write removed). W^X enforcement.
9. **Init** — `scarlet_lsm_init()` is called. Failure triggers rollback (VM unmap + page free).
10. **Symbol export** — Module's `STB_GLOBAL`/`STB_WEAK` defined symbols are registered in `SymbolRegistry`.
11. **Registry** — Module is added to `MODULE_REGISTRY` with its ID, name, and dependencies.

On failure at any step, all allocated pages are freed and VM mappings are removed (rollback).

## Symbol Resolution

### Kernel Symbol Table

Kernel symbols are collected from two linker sections:

- `.scarlet_ksyms` — Populated by the `export_symbol!` macro at compile time
- `.lsm_symbols` — Populated by `generated_symbols.rs`, generated from `nm` output of the kernel binary

Both sections are placed after `__KERNEL_SPACE_END` in the linker scripts (no PHDR).

**Two-pass build**: The kernel must be built first, then `nm` extracts symbols, `generated_symbols.rs` is regenerated, and the kernel is rebuilt. The generated file is marked `assume-unchanged` in git.

### Crate Hash Stripping

Rust mangled names include crate disambiguator hashes (e.g. `NtC<12hexchars>_`) that vary with `opt-level`, `rustflags`, etc. The `strip_crate_hash` function in `symbol.rs` normalizes these: `NtC` followed by exactly 12 ASCII-alphanumeric characters is replaced with just `NtC`.

### Resolution Order

The `SymbolRegistry` searches entries in reverse order (last-registered first). This means:
- Kernel symbols are the baseline
- Later-loaded module symbols override earlier ones

Each `RegistryEntry` tracks `{ name, addr, module_id }` where `module_id: None` indicates a kernel symbol.

## Architecture Details

### RISC-V 64

- Module VM region: `0xffffffff90000000` (256MB)
- Supported relocations: `R_RISCV_32`, `R_RISCV_64`, `R_RISCV_BRANCH`, `R_RISCV_JAL`, `R_RISCV_CALL`/`R_RISCV_CALL_PLT`, `R_RISCV_PCREL_HI20`, `R_RISCV_PCREL_LO12_I`/`S`, `R_RISCV_HI20`, `R_RISCV_LO12_I`/`S`, `R_RISCV_ADD32`, `R_RISCV_SUB32`, `R_RISCV_SET16`, `R_RISCV_SET32`, `R_RISCV_32_PCREL`
- Unsupported: RVC (`R_RISCV_RVC_BRANCH`, `R_RISCV_RVC_JUMP`), `R_RISCV_ALIGN`
- icache: `fence.i`

### AArch64

- Module VM region: `0xffffffff81000000` (128MB of BL range from kernel)
- Supported relocations: `R_AARCH64_ABS64`, `R_AARCH64_ABS32`, `R_AARCH64_PREL32`, `R_AARCH64_ADR_PREL_PG_HI21`, `R_AARCH64_ADR_PREL_LO21`, `R_AARCH64_ADD_ABS_LO12_NC`, `R_AARCH64_LDST8_ABS_LO12_NC`, `R_AARCH64_JUMP26`, `R_AARCH64_CALL26`, `R_AARCH64_MOVW_UABS_G0`/`G0_NC`, `R_AARCH64_MOVW_UABS_G1_NC`, `R_AARCH64_MOVW_UABS_G2_NC`, `R_AARCH64_MOVW_UABS_G3`
- icache: DC CVAU per range + IC IALLU + DSB ISH + ISB

## Syscall Interface

| Syscall | Number | Arg | Return |
|---|---|---|---|
| `sys_lsm_load` | 1200 | `arg0`: `*const u8` path in userspace | `LsmErrorCode` as `usize` |
| `sys_lsm_unload` | 1201 | `arg0`: `module_id` as `u64` | `LsmErrorCode` as `usize` |
| `sys_lsm_list` | 1202 | `arg0`: buffer ptr, `arg1`: buffer size | count of entries written |

### lsm_list ABI

Each entry is 264 bytes:
- Bytes 0-7: `u64` module ID (little-endian)
- Bytes 8-263: NUL-terminated module name (256 bytes)

### Error Codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `Success` | Module loaded/unloaded successfully |
| 1 | `InvalidPath` | Path not found or not a file |
| 2 | `InvalidElf` | ELF parsing failed |
| 3 | `NoMemory` | Page allocation or VM mapping failed |
| 4 | `RelocationError` | Relocation processing failed |
| 5 | `NoInit` | `scarlet_lsm_init` symbol not found |
| 6 | `InitFailed` | `scarlet_lsm_init` returned `Err` |
| 7 | `BuildInfoMismatch` | `RUSTC_VERSION;TARGET` doesn't match kernel |
| 8 | `NotFound` | Module ID not found (unload) |
| 9 | `PermissionDenied` | Other module depends on this module (unload) |
| 10 | `MissingDependency` | Required dependency not loaded |

## Troubleshooting

**BuildInfoMismatch (error 7)**: Module was built with a different rustc version or target than the running kernel. Rebuild the module with the same toolchain, or rebuild the kernel.

**MissingDependency (error 10)**: A dependency declared in `module.toml` is not loaded. Load dependencies first (or use `lsm_load` which auto-resolves), or check that the dependency module name matches exactly.

**RelocationError (error 4)**: The module references an undefined symbol, uses an unsupported relocation type (e.g. RVC on RISC-V), or has a malformed relocation. Check that all referenced kernel functions are accessible.

**InvalidElf (error 2)**: The `.lsm` file is not a valid ELF64 relocatable object. Ensure the module was built with `--emit=obj` via cargo-scarlet, not as a cdylib or executable.

**Stale kernel symbols**: After modifying kernel APIs, re-extract symbols by rebuilding the kernel (the two-pass build regenerates `generated_symbols.rs`). If modules fail to resolve symbols that should exist, the symbol table may be outdated.
