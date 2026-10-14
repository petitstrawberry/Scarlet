# Native TLS header and C errno

Scarlet's native CRT, Rust std, loader, and libc share a per-thread mapping at
the architectural thread pointer: `TPIDR_EL0` on AArch64 and `tp` on RV64.
The [layout definition](../../user/lib/scarlet-abi/src/tls.rs) and
[C errno accessor](https://github.com/petitstrawberry/scarlet-sdk/blob/e393a33e576ca0d4be8cbb947748a4975405a91f/scarlet-libc/src/errno.rs) implement this
contract. See the [libc integration guide](https://github.com/petitstrawberry/scarlet-sdk/blob/e393a33e576ca0d4be8cbb947748a4975405a91f/scarlet-libc/README.md)
for the supported C surface and build workflow.

The header and cleanup area are the native runtime ABI. On AArch64, the
space between them also holds a main executable's static ELF TLS block. This
does not supply dynamic TLS modules, TLS relocations, or RV64 ELF TLS.

## Layout

`NativeTlsHeader` has C layout and starts at the thread pointer. On both
supported 64-bit targets it occupies 16 bytes:

| Offset | Type | Field | Initial value |
| --- | --- | --- | --- |
| 0 | `usize` | `namespace_head` | 0 |
| 8 | `u32` | `magic` | `0x53435401` |
| 12 | `i32` | `errno` | 0 |

The generic offsets are 0, `sizeof(usize)`, and `sizeof(usize) + 4`.
The corresponding 32-bit layout would occupy 12 bytes; this does not establish
32-bit libc support. Namespace nodes and their ownership remain private to
the matching Rust std implementation.

The existing thread-exit cleanup area starts at
`1024 * sizeof(usize)`, and the mapping size is that offset plus 4096 bytes.
On AArch64 and RV64 these are 8192 and 12288 bytes respectively. Adding the
header does not move the cleanup area or change the mapping size. Code using
the mapping must preserve the header and cleanup metadata.

## AArch64 executable ELF TLS

The header occupies the 16-byte AArch64 TCB. A main executable's `PT_TLS`
block starts at `align_up(16, p_align)` and must end before the cleanup area
at 8192. Alignment must be a power of two at most 4096, with zero virtual
address alignment residue. The shared `aarch64_elf_tls_offset` helper checks
this bounded layout. Initialized bytes must match a readable file-backed
`PT_LOAD`; duplicate or malformed TLS headers are rejected.

The kernel records the main's template and copies its initialized bytes,
followed by zeroed `.tbss`, when `SetTls` publishes a mapping and when
`Clone(SET_TLS)` creates a thread. Copies use checked userspace access. The
loader refreshes the main mapping after relocation and before constructors,
so pointers in `.tdata` use their relocated values. Later `dlopen` calls do
not reinitialize the main's TLS. Startup preserves the existing mapping.

AArch64 local-exec accesses relaxed by the linker therefore use independent
per-thread storage. `dlsym` of a main-executable TLS symbol returns its address
in the calling thread. TLS in dynamically loaded DSOs and all TLS relocation
models remain explicitly unsupported.

## Initialization and lifetime

`__scarlet_start` initializes the main thread's header before environment
initialization and before constructors. If the loader subsequently enters
`__scarlet_start` for the executable, startup preserves the existing valid
mapping, namespace head, and errno value. It must not reset a header that has
already been initialized.

Startup reads the architectural thread pointer. A newly created main-thread
mapping is published through `SetTls`, keeping the live architectural pointer
and the kernel's saved TLS state consistent. A kernel `GetTls` result alone
is not a substitute for reading the live pointer when older code may have
written that pointer directly.

For a spawned thread, the creator initializes an independent mapping before
calling native clone with `SET_TLS`. The child must have a valid header before
executing user code. Custom entry points and foreign thread creators must
honor the same layout, initialization, publication, and cleanup contract;
inheriting another thread's mapping would alias both errno and Rust TLS.

The main thread's mapping lives until process exit. A spawned thread retains
its header throughout Rust TLS destructor processing: clearing the namespace
head does not clear errno or invalidate its address. Native thread-exit
cleanup unmaps the full mapping after destructors. An errno pointer must not
be used after its owning thread exits.

## Access and compatibility

On Scarlet, `__errno_location` reads the architectural pointer, validates the
magic, and returns the address of the header's errno cell. Its normal access
path performs no allocation or syscall, including the first access and access
from TLS destructors. A missing pointer or mismatched magic aborts; the C
accessor has no lazy allocation fallback. Runtime initialization and failure
handling must not depend on accessing errno before the header exists.
Rust std's `std::io::Error::last_os_error` reads the same errno cell.

This guarantee is specific to errno storage. Rust's namespace-based TLS can
still allocate its private namespace nodes and values. The header does not
make arbitrary Rust TLS operations allocation-free.

Use a matching CRT, libc, Rust std, loader, executable, and Rust DSOs together.
Rust std must validate the header in its shared TLS-base path as well as libc
validating it in the C accessor. Older inline-slot and namespace-based runtimes
without this header are incompatible: they can overwrite its fields or create
threads without its magic. Matching only the C library is insufficient, and
replacing an initialized native TLS mapping is outside this contract.

## Bounded validation

The static executable TLS changes pass 30 host ABI tests, 25 loader-core tests,
and 10 dynamic-runtime tests in release mode. The new checks cover reserved
mapping bounds, malformed templates and symbols, initialization after
relocation, thread-relative symbol lookup, and rejection of dynamic DSO TLS.
These host checks do not execute target TLS instructions; the Myrica Skia
Vulkan probe additionally checks initialized C++ TLS and Rust namespace TLS
across four native threads before drawing.

The release probe passed this four-thread check on AArch64 with HVF and four
guest CPUs. Each child observed the C++ initializer value 17, mutated its own
TLS and errno, and left the parent's C++ value, Rust TLS value, and errno intact.


The bounded AArch64 guest checks covered constructor access,
new-thread isolation, destructor access, a controlled allocation-backend null
result, and an ordinary C `main` linked with Scarlet CRT and libc that exits
with status 43. The recorded AArch64/HVF run also passed full native
compilation, proc-macro execution, and native filesystem verification. RV64
coverage is cross-build and ELF audit only.

These results do not establish signal-handler safety, recovery from physical
memory exhaustion, installed SDK acceptance, or a complete C runtime. They
apply to the recorded runtime artifacts; an installed distribution requires
separate validation.
