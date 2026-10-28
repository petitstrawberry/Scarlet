# Native OpenJDK on Scarlet

This builds OpenJDK 25's actual HotSpot VM with an **AArch64 C1 JIT** and a `java.base` runtime for
native Scarlet AArch64. The executable uses the Scarlet SDK and native syscall
ABI (ELF OSABI `0x53`). It does not require a Linux userspace or Linux ABI zone.

The port is maintained in [petitstrawberry/jdk25u, branch scarlet-25](https://github.com/petitstrawberry/jdk25u/tree/scarlet-25),
forked from `openjdk/jdk25u` at `jdk-25.0.5+8`. `sources.json` pins the port
commit and archive SHA-256, plus libffi 3.4.8. Change the OS port in that fork,
push a commit, then update the commit, URL, directory and checksum here.
There is no second local copy of the port or patch series to synchronize.
The previous Java 21 port remains in [jdk21u/scarlet-21](https://github.com/petitstrawberry/jdk21u/tree/scarlet-21).
The driver still accepts its pinned 21 manifest with a host JDK 21. JDK 25 uses
the upstream `hotspot-static-libs` target instead of the removed configure-time
static-build switch; packaging derives the runtime version from upstream sources.

## Build and run

Use this repository's Nix development shell and a host **JDK 25**. The boot JDK
must match the build host and the selected source major version. On macOS a JDK home may be inside `Contents/Home`.

```sh
nix develop
python3 tools/openjdk/build.py --target java --boot-jdk /path/to/host-jdk-25
python3 tools/openjdk/tests/validate.py \
  --build-receipt .build/openjdk/java.json \
  --boot-jdk /path/to/host-jdk-25
```

The validator builds a separate kernel and native init, compiles the Java test,
creates an initramfs, and boots a disposable, diskless QEMU guest. On Apple
Silicon, add `--accel hvf` for hardware acceleration; the default is TCG. Guest
output and the PASS/FAIL receipt are in `.build/openjdk-guest/java-qa/`.

C1 is the default and reports `OpenJDK 64-Bit Client VM`. Use
`build.py --target java --jvm-variant zero --boot-jdk ... --output .build/openjdk-zero`
to build the Zero interpreter separately. The validator detects the variant
from the receipt and requires JIT compilation evidence for C1.

The build receipt `.build/openjdk/java.json` identifies `artifact`, `java_home`
and `image_root`. Install the contents of `image_root/bin` and `image_root/usr`
into the Scarlet root filesystem. The layout is:

```text
/bin/java
/usr/lib/jvm/openjdk-25/lib/modules
/usr/lib/jvm/openjdk-25/release
```

Compile application classes with a host JDK 25 and copy them into the guest,
then run `java -cp /app Main`. The launcher supports a main class, VM options,
`-cp`/`-classpath`/`--class-path`, and `-version`/`--version`. Its default class
path is `.`; the default heap is 16 MiB initial / 64 MiB maximum. `JAVA_HOME`
can override `/usr/lib/jvm/openjdk-25`.

Builds are keyed by source, helper, toolchain and boot JDK identities. Downloads
are checksum verified and edited source trees are rejected without resetting
them. Use `--output` for a separate owned output directory. Receipts distinguish
compilation/ELF inspection from actual guest execution.

For narrower diagnosis, `build.py --target hotspot` builds only `libjvm.a`;
`--target runtime-probe` builds the libffi/pthread/memory syscall probe and
`--target runtime-support-probe` builds the environment/C++ TLS destructor probe.
Run those native binaries with `tests/run-runtime.py`; building a probe alone
does not validate guest execution. See each command's `--help`.

## Verified scope and limits

The Java smoke test executes `java -version`, UTF-8 arguments, floating-point
operations, overlapping array copy, four platform threads, atomics, thread-local
storage, 128 MiB of allocations with Serial GC, a 98,317-byte file round trip,
and normal JVM destruction/process exit. The memory probe also checks guarded
loads from invalid, unmapped, protected and lazy mappings.

For C1, a second process uses `-Xcomp`, disables inlining of the tested methods,
and requires compilation log entries for arithmetic, field load and recursive
methods. It checks their results, catches an implicit NullPointerException and
a stack-guard StackOverflowError, then executes compiled code again. The native
fault probe checks GPR/SIMD/NZCV preservation, invalid return contexts, and writes
to PROT_NONE/read-only pages.

A separate C1 stress process holds live objects and arrays in compiled frames
on four platform threads while a controller requests eight full collections.
Each worker recovers from eight null faults and one stack overflow. The runner
requires compilation entries, eight GC log entries from that process, intact
references after collection, and successful termination of all four workers.

This is an initial headless `java.base` port, not a complete or TCK-certified
JDK. C1 is implemented; C2 and virtual threads/continuations are unavailable.
Reserved-stack annotations are also unavailable (`StackReservedPages=0`). AWT, dynamic
JNI libraries, JVMTI/agents, JFR, process creation and the launcher's `-jar`/`-m`
modes are unavailable. Networking, NIO and additional modules
have not been validated; their native libraries are not part of this image.
Process RSS and per-page residency queries are unavailable; detailed NMT
residency is unsupported. Memory disclaim remains a reclamation hint; uncommit
and release discard pages. HotSpot disables stack protection because the SDK
has no stack-protector runtime. GC log timing fields can be blank because the current SDK C formatter lacks
floating-point formatting. OpenJDK and its notices retain their upstream
licenses; the build uses the separately licensed Scarlet SDK and libffi.

## Scarlet prerequisites

This change adds native syscall 53, `GetMemoryInfo`, returning the 24-byte
`RawMemoryInfo` record (page size, managed pages, free pages), and syscall 704,
`MemoryReadWord`, used for HotSpot's fault-tolerant SafeFetch. The latter holds
mapping/page-table guards during an aligned 4-byte or native-word RAM load and
returns the caller's fallback for invalid addresses or permissions. It rejects
device memory and can fault in readable lazy pages.

On AArch64, syscalls 645/646 register a per-thread alternate-stack synchronous
fault handler and restore its versioned context. The kernel retains FP/SIMD and
TLS state and restores only user NZCV flags. Nested handler faults are fatal;
registrations are reset on thread cloning and successful exec. Exception delivery
uses `AbiModule::handle_user_exception` for the ABI owning the faulting PC; the
AArch64 frame codec and restoration live under `abi/scarlet/aarch64/fault.rs`. Lazy mapping rejects denied accesses
so protection faults reach the handler instead of repeatedly reinstalling the
same inaccessible page.

Init opens its shared console description for both reading and writing, so
native C writes through duplicated stdout/stderr handles have write permission.

Host-side build-driver checks:

```sh
python3 -m unittest discover -s tools/openjdk/tests -p 'test_*.py'
cargo test --manifest-path user/lib/scarlet-abi/Cargo.toml --test native_contract
```

## Minecraft target

The client target is **Minecraft Java Edition 26.3**, using its experimental
Vulkan renderer. Its [official metadata](https://piston-meta.mojang.com/v1/packages/5df1795e16eea19d75b923d66ecca9ce183353c5/26.3.json)
requires Java 25 and LWJGL 3.4.3. The [26.3 release notes](https://www.minecraft.net/en-us/article/minecraft-java-edition-26-3)
confirm SDL3 window/input support. This supersedes the earlier Java 21 / 1.21.11
candidate, which would require an OpenGL path.

The official client has SHA-1 `e877b6a07acd633fb3bb475002175cec036e7b87`
and size 41,483,720 bytes. Downloaded files are local validation inputs, not
repository content. `net.minecraft.client.main.Main` uses class-file version
69 (Java 25). No account credentials are needed to retrieve the official files.

The current runtime has not launched Minecraft. Remaining work includes
JAR/reflection/NIO compatibility, dynamic JNI loading through `scarlet-ld`,
additional JDK modules, and native LWJGL/SDL3/shaderc/audio bindings. Analysis of
the client alone (without third-party libraries) requests `java.desktop`,
`java.management`, `java.naming`, `java.net.http` and `jdk.jfr` in addition to
`java.base`; this is a lower bound, not a complete dependency audit.

Minecraft [documents Vulkan 1.2 plus dynamic rendering and push descriptors](https://www.minecraft.net/en-us/article/minecraft-java-edition-26-2).
Scarlet's pinned SGFX ICD manifest currently advertises 1.1.0. Its existing
loader/offscreen drawing tests do not establish Minecraft feature compatibility;
the ICD needs a feature audit and implementation work before a client launch
can succeed. Graphics initialization must report missing features instead of
advertising support that the driver cannot execute.
