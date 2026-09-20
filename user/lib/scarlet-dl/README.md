# Scarlet Native dynamic runtime

`scarlet-ld` statically embeds this crate and keeps its `LoaderContext` alive
after transferring control to the application's original entry. Its exported
`dlopen`, `dlsym`, `dlclose`, and `dlerror` addresses are inserted into that
same context before startup relocation. Applications import those C symbols;
they must not statically link a separate copy of this crate.

The initial runtime intentionally supports only eager global loading:

```c
#include "scarlet_dl.h"
void *library = dlopen("/system/lib/plugin.so", RTLD_NOW | RTLD_GLOBAL);
void *symbol = library ? dlsym(library, "plugin_entry") : 0;
```

- A pathname containing `/` is opened directly. Bare dependency names search
  the requesting object's directory, `/system/lib`, then `/lib`. Bare runtime
  `dlopen` names search `/system/lib` then `/lib`. Environment search-path
  overrides and ELF RPATH/RUNPATH are not implemented.
- `dlopen(NULL, RTLD_NOW | RTLD_GLOBAL)` returns a process handle whose scope includes subsequently loaded global objects.
  `dlsym(NULL, name)` searches the global namespace; an object handle searches
  that object's dependency closure. Missing names produce `dlerror` text.
- Constructors run dependency-first, once, after relocation and protection.
  The executable entry remains responsible for its own constructors, as
  Scarlet's native std `_start` already is. Constructor reentry and overlapping
  loader operations report a busy error; they do not wait or deadlock.
- `dlclose` invalidates its handle, but objects remain pinned until process
  exit. Previously returned symbols therefore retain their mappings. Unload
  and finalizers are not supported. Handles are never reused.
- Only `RTLD_NOW | RTLD_GLOBAL` is accepted. Lazy/local loading, interposition
  controls, `RTLD_NEXT`, ELF TLS, IFUNC, symbol versions, and unsupported
  relocations fail explicitly. `dlerror` is per-thread and consumes each error.
- Native `MemoryMap` reserves zeroed writable, non-executable image pages;
  `MemoryProtect` (702) installs final permissions and synchronizes executable
  pages. Writable executable pages are rejected.

The interpreter must be built using the Scarlet **native std fork**, not the
older JSON/no-std user target. Its build script produces a static ET_EXEC at
`0x40000000`, with `_scarlet_ld_start` preserving the kernel's original SP.
That shim enters native std startup. The runtime requires and consumes the kernel-owned
`AT_EXECFD` native file handle, seeks to the start, reads the exact executable,
and closes the handle on both success and failure. `AT_EXECFN`, when present,
is an optional verified origin path and is never reopened. Without a path, the main uses a synthetic identity and dependencies
search only `/system/lib` and `/lib`. Adoption validates bounded load ranges,
derives the bias from `PT_LOAD`, and checks every file-backed readable byte
against the still-unrelocated kernel mapping. Unreadable file-backed main
segments are rejected.

The final trampoline restores the
original SP and argc/argv registers before branching to `AT_ENTRY`. AArch64
and RISC-V64 entry shims are provided; availability of a matching std sysroot
is required to build either target.

This runtime is a bring-up layer. It does not supply a C library or LLVM's
dependencies, stable Rust ABI, native ELF TLS, or a complete rustc host port.
