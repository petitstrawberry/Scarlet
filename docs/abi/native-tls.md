# Native TLS header and C errno

Scarlet's native CRT, Rust std, loader, and libc share a per-thread mapping at
the architectural thread pointer: `TPIDR_EL0` on AArch64 and `tp` on RV64.
The [layout definition](../../user/lib/scarlet-abi/src/tls.rs) and
[C errno accessor](../../user/lib/scarlet-libc/src/errno.rs) implement this
contract. See the [libc integration guide](../../user/lib/scarlet-libc/README.md)
for the supported C surface and build workflow.

This mapping is a native runtime ABI, not ELF TLS. It does not implement or
replace ELF TLS relocation, module, or thread-pointer conventions.

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

The [AArch64 errno evidence](../../tools/native-rustc/evidence/2026-09-22-libc-errno-aarch64.json)
tracks the tested artifacts and bounded guest checks: constructor access,
new-thread isolation, destructor access, a controlled allocation-backend null
result, and an ordinary C `main` linked with Scarlet CRT and libc that exits
with status 43. The recorded AArch64/HVF run also passed full native
compilation, proc-macro execution, and native filesystem verification. RV64
coverage is cross-build and ELF audit only.

These results do not establish signal-handler safety, recovery from physical
memory exhaustion, installed SDK acceptance, or a complete C runtime. They
apply to the recorded runtime artifacts; an installed distribution requires
separate validation.
