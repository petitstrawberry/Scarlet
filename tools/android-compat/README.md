# Android compatibility: two execution paths

Both paths have runnable, isolated fixtures. They answer different questions;
neither fixture certifies general APK compatibility.

| | Android in a VM | Direct Android userspace |
|---|---|---|
| Execution | APK → Android framework/ART → guest Linux → crosvm → Scarlet | Android ELF → Bionic/linker → Scarlet Android ABI (composes Linux ABI) |
| Reused Android facilities | Guest Binder, Android init, framework and HALs | Stock Bionic/linker and libbinder on Scarlet's Binder adapter |
| Main work ahead | Finish framework boot, then connect display/input to SWS | Zygote child specialization, Android lifecycle management, and graphics integration |
| Current rendering | Cuttlefish ANGLE/SwiftShader, headless virtio-gpu stub | No Android graphics stack yet |
| Fixture | [Cuttlefish project](../../projects/aarch64-limine-crosvm/README.md) | This directory |

The direct fixture uses Android 13/API 33 while the pinned VM uses Android 17.
These are compatibility checkpoints, not a controlled performance comparison.
Continue the VM track toward a real APK while the direct track advances through
Bionic → Binder client/server → ART/JNI → a minimal framework application.

## Direct Bionic execution

Verified on 2026-10-29: **eighteen cases pass** with `--binder` on Scarlet/AArch64 with outer QEMU
HVF, Android NDK r25b targeting API 33, and an Android 13 ARM64 runtime APEX
from the locally installed SDK image. No guest Linux kernel is involved.

Each case runs in a fresh process and must produce both child `BEGIN`/`OK`
records and exit status zero. The host also requires the final suite marker.

| Case | Static | Dynamic | Checks |
|---|---|---|---|
| libc | PASS | PASS | Constructors, TLS, auxv, malloc, proc-exe metadata, tagged access, main-thread stack attributes and resource limits |
| thread | PASS | PASS | pthread create/join, TLS, mutex, futex compare/requeue/wake and timeout cleanup after requeue |
| shared | PASS | PASS | memfd and coherent MAP_SHARED mappings |
| signal | PASS | PASS | Handler return, pending-signal consumption, sigwait wake, timed wait and unrelated-signal interruption |
| dynamic | — | PASS | dlopen/dlsym/dlclose, DSO constructor and native ELF TLS |
| exec | PASS | PASS | Sparse FD numbers, file flags, CLOEXEC and Android device dispatch survive exec |
| xattr | PASS | PASS | All twelve path/fd/no-follow calls, binary values, limits, atomic flags, rename and unlink identity |
| mremap | PASS | PASS | Fixed relocation, in-place/moving growth, lazy pages, protection, fork isolation, zeroed regrowth and 512 Bionic atexit callbacks |
| binder | PASS | PASS | Three-process object forwarding, owner roundtrip, strong/weak reference lifetimes, malformed objects, 64-object boundaries, cleared buffers, peer closure, fork isolation, poll and epoll readiness |

An additional static probe explicitly selects `linux-aarch64` and verifies that
none of the three Binder aliases is exposed. This is the eighteenth case.

From the repository root, inside `nix develop`:

```sh
cargo scarlet build --release --project projects/aarch64-limine-crosvm
python3 tools/android-compat/build-probes.py \
  --ndk "$ANDROID_NDK_HOME" --api 33 --output .build/android-compat/probes
python3 tools/android-compat/run-direct.py \
  --kernel projects/aarch64-limine-crosvm/bsp/target/aarch64-unknown-none-elf/release/scarlet \
  --probes .build/android-compat/probes \
  --output .build/android-compat/static-run --accel hvf
```

Use `--accel tcg` on hosts without HVF. The direct fixture does not require
nested virtualization. It needs the development environment's matching AArch64
EFI code/variables; see the crosvm runner documentation for firmware variables.
Every run uses a new output directory, private disks and a private EFI variable
store. `result.json`, `serial.log`, `commands.json`, `inputs.json` and
`probes.json` retain the results and input identities.

For dynamic execution, supply **runtime** libraries, not NDK link stubs. Export
`com.android.runtime.apex` from an ARM64 Android image matching the API level,
then extract its uncompressed ext4 payload without privileged mounts:

```sh
python3 tools/android-compat/extract-runtime.py \
  --apex /path/to/com.android.runtime.apex --output .build/android-compat/runtime
python3 tools/android-compat/run-direct.py \
  --kernel projects/aarch64-limine-crosvm/bsp/target/aarch64-unknown-none-elf/release/scarlet \
  --probes .build/android-compat/probes \
  --runtime-root .build/android-compat/runtime \
  --output .build/android-compat/dynamic-run --accel hvf --binder
```

The extractor uses read-only `debugfs`, checks ARM64 ELF headers, and exports
only linker64 and four Bionic libraries. It records the APEX and file hashes.
Compressed CAPEX and EROFS payloads are not supported. The minimal runtime has
no generated `/linkerconfig/ld.config.txt`; the Android 13 linker prints a
warning and uses its fallback namespace with the explicit library search path.
It does not provide Android linker namespace isolation or framework services.
Generated Android binaries and SDK images remain local artifacts.

### Shield identities and stock Zygote control

Shield stores process principals, groups, retained identities and independently
revocable authority. Linux/Android translate real/effective/saved UID/GID,
supplementary groups, `PR_SET_KEEPCAPS` and supported capability bits (`SETUID`,
`SETGID`, `SYS_ADMIN`). Unknown capabilities and inheritable grants are rejected.
Fork copies credentials; exec preserves identities, resets saved IDs and removes
retained authority on a non-root Linux exec. Binder reports the caller's effective
UID; UNIX peers and queued messages retain identity snapshots.

`unshare(CLONE_NEWNS)` snapshots mount Views in a process-side namespace while
retaining Environment identity and its Binder domain. Fork and exec preserve the
mapping. Private/slave mount propagation requests are accepted on existing mounts;
there are no shared propagation peers in the current VFS.

The Bionic fixture checks ID transitions, denied privilege recovery, groups,
capability drops, exec auxiliary vectors, socket credentials and Binder calls
across a mount split. These primitives do not yet provide full application
confinement: VFS ownership/DAC and general capabilities still require work.
Shield also retains immutable syscall constraints across fork, spawn and exec.
The AArch64 Linux adapter verifies seccomp cBPF and enforces errno, SIGSYS and
termination actions before dispatch, including native-call escape attempts.
Bionic exercises rejected writes without side effects, caught SIGSYS, per-thread
filters, fatal actions, stacking and fork/exec inheritance. Strict mode, TSYNC
and listener creation remain unsupported; RISC-V installation is rejected until
its signal adapter supports synchronous SIGSYS metadata.

`build-zygote-probe.py` and `run-direct.py --zygote-probe` verify stock
`app_process64 -Xzygote ... --zygote` with lazy preload. A userspace supervisor
creates the named control sockets, requests the ABI list and PID, then stops and
reaps its owned Zygote. Success requires both correct protocol responses and
cleanup, enforcing Shield status and the daemon's inherited label; no `LD_PRELOAD`
is used. Child specialization and Activity launch are
separate milestones.

```sh
python3 tools/android-compat/build-zygote-probe.py \
  --ndk "$ANDROID_NDK_HOME" --framework-inputs /path/to/framework-inputs \
  --runtime-root /path/to/art-runtime --stock-inputs /path/to/stock-inputs \
  --output .build/android-compat/zygote-probe
python3 tools/android-compat/run-direct.py --kernel /path/to/scarlet \
  --zygote-probe .build/android-compat/zygote-probe \
  --framework-inputs /path/to/framework-inputs --runtime-root /path/to/art-runtime \
  --stock-inputs /path/to/stock-inputs --accel hvf \
  --output .build/android-compat/zygote-run
```

### Preparing the ART interpreter checkpoint

`prepare-art.py` prepares **real ARM64 ART**, including Bionic, core DEX jars,
ICU data, timezone data and the ELF dependency closure. It accepts exactly the
Android 13/API 33 revision 2 SDK image below. Its SHA-256 pin fixes the GPT and
liblp extent offsets used to extract the system ext4 partition. It is not a
general Android disk parser. Extraction uses read-only `debugfs`; CAPEX packages
are unpacked through `original_apex`, with no privileged mounts. No NDK shared
libraries are used as runtime inputs.

| Input | Source/version | SHA-256 |
|---|---|---|
| SDK system image | [ARM64 API 33 revision 2](https://dl.google.com/android/repository/sys-img/android/arm64-v8a-33_r02.zip), Android build `TE1A.220922.034` / `10940250` | `55ba9a20d473dd5351c77342016956342c40375f3e073b65a43dc9db13ccc5c6` |
| D8 build tools | [Build Tools 33.0.2, Linux](https://dl.google.com/android/repository/build-tools_r33.0.2-linux.zip) | `ff19ccbe3a5098b5dd10722063ebcd40218a30d23b50133e386fc39ea5240e79` |
| Bionic probe compiler | [NDK r25b, Linux](https://dl.google.com/android/repository/android-ndk-r25b-linux.zip), `25.1.8937393` | `403ac3e3020dd0db63a848dcaba6ceb2603bf64de90949d5c4361f848e44b005` |

From `nix develop`, with a JDK installed (`JAVA_HOME`, including Java 8 release
symbols) and SDK Build Tools unpacked:

```sh
python3 tools/android-compat/prepare-art.py \
  --sdk-image /path/to/arm64-v8a-33_r02.zip \
  --output .build/android-compat/art-runtime
python3 tools/android-compat/build-dex.py \
  --build-tools /path/to/sdk/build-tools/33.0.2 \
  --output .build/android-compat/dex
python3 tools/android-compat/run-direct.py \
  --kernel projects/aarch64-limine-crosvm/bsp/target/aarch64-unknown-none-elf/release/scarlet \
  --runtime-root .build/android-compat/art-runtime \
  --art-dex .build/android-compat/dex/classes.dex \
  --output .build/android-compat/art-run --accel tcg --timeout 180
```

All commands require a fresh output directory. `art-runtime.json` records the
SDK URL/hash/build properties, package/APEX/payload hashes, every output hash,
ELF dependencies, and the explicit library/boot-class search paths. `build.json`
records the Java/compiler/D8 versions, commands, source hash and DEX hash. The
plain Java probe computes 42 and prints `SCARLET_ART_BEGIN`, `Scarlet ART: 42`
and `SCARLET_ART_OK`, then returns normally. It has no Activity or framework
dependencies. The runtime tree has no boot image or linker namespace config;
these inputs target a core-only, image-less interpreter experiment.

`--art-dex` replaces the Bionic suite and does not need `--probes` or the NDK.
The host checks runtime hashes before and after staging, validates the DEX
header/checksums, saves `art-launch.json`, and compiles those same arguments and
environment entries into the native init. The child is launched with `-Xint`,
JIT disabled, an intentionally absent boot image and `-Xnoimage-dex2oat`.
A native logger thread forwards `/dev/kmsg` while the parent waits for the ART
process, including startup failures and hangs. Success requires ordered Java
BEGIN/output/OK records, native wait success, exit status zero and the final
suite marker. `evidence.py --art` independently rechecks a serial log. These
runner paths have host orchestration tests; the native launcher has only been
syntax/format checked in Cloud, not built or executed there.

Cloud checkpoint on 2026-10-28: input extraction and DEX compilation succeeded
on x86_64 Linux, using OpenJDK `21.0.12.1` and D8 `3.3.20-dev+aosp5`. The DEX is
1,296 bytes, SHA-256 `aaba12744df03510040942b60e66af7959ee1ce1fbdc5124a4304f757d3be2d1`.
Host preparation used `debugfs 1.47.2` and host Python because the project Nix
environment was blocked. Nix `2.24.11` in `nixos/nix:2.24.11` could fetch flake
sources after using the Cloud CA bundle, but `cache.nixos.org` and
`scarlet-rust-toolchain.cachix.org` returned HTTP 403. Its source-build fallback
then failed to fetch `https://ftpmirror.gnu.org/bash/bash-5.3.tar.gz`.
**No Cloud Scarlet build, ART startup, Java execution on Scarlet, or ABI fix
has been verified at this checkpoint.** These host checks do not replace the
earlier local Bionic or kernel tests.

An additional **host Linux user-mode emulation diagnostic, not Scarlet**, used
QEMU `10.0.13` to check the real `dalvikvm64` inputs. It first aborted because
`/system/etc/public.libraries.txt` was missing. Preparation now preserves the
image's original list and its preload dependencies (including Android `@`
versioned library names). The resulting tree contains 267 hashed files and
253 ELF dependency records, about 120 MiB. A second startup then aborted with
`Failed to get system namespace for loading libandroid.so`; no Java marker
was printed and the command exited 1. Both logs are retained in
[the Cloud checkpoint](checkpoints/2026-10-28-cloud/verification.json).

The image's unmodified static `linkerconfig` was also inspected and executed
as a host diagnostic. With the fixture's absent property/APEX activation state,
it generated only a legacy configuration with no ART/system exported namespace.
The owner subsequently approved reproducing stock Android property/APEX inputs
and using Android's original configuration generator. No custom namespace
policy was installed. The current Scarlet ART runner remains a startup
diagnostic, not a passing Java fixture.

### Stock property/APEX preparation and host diagnosis

`build-properties.py` builds a static ARM64 bootstrap which calls the original
AOSP property parser/serializer and Bionic property-area APIs. Sources are
pinned by URL, tag and SHA-256 in `art/property-sources.json`: the Android 13
`android-13.0.0_r75` serializer/parser and the unmodified Android 10
`android-10.0.0_r47` libbase string utility used by that parser. NDK r25b static
Bionic writes the areas; no NDK shared link stubs enter the runtime. This helper
creates a one-time property snapshot, not a property service or Android init.
It fails on property initialization errors and reads every added value back.

```sh
nix develop -c python3 tools/android-compat/build-properties.py \
  --ndk /path/to/android-ndk-r25b \
  --source-cache .build/android-property-sources --download \
  --output .build/android-property-helper
nix develop -c python3 tools/android-compat/prepare-stock-art.py \
  --sdk-image /path/to/arm64-v8a-33_r02.zip \
  --runtime-root .build/android-art-runtime \
  --property-helper .build/android-property-helper \
  --output .build/android-stock-inputs
```

The output is an **overlay of inputs**, tied by hashes to the prepared runtime.
It contains the original system/vendor property contexts, 216 literal build
properties from system/system_ext/vendor/product in init's precedence order,
27 factory APEX manifests and their original linker protobufs, VNDK/sanitizer
library lists, and the unmodified runtime APEX `linkerconfig`. Factory activation
XML is derived from the original package manifests, without namespace rules.
Library-directory and executable-directory presence is preserved because the
generator examines it. Only the selected runtime APEXes' libraries are copied;
metadata for other packages does not imply that their services can run. The
preparation also preserves image library aliases and system/private APEX
dependency copies required by isolated namespaces. Every added file and alias,
the exact partition extents and source package hashes are recorded.

An optional **host Linux diagnostic**, explicitly separate from Scarlet, uses
Docker and a static host `qemu-aarch64`. Supply an already available image or use
the script's pinned `nixos/nix` digest. Containers have networking disabled.

```sh
nix develop -c python3 tools/android-compat/diagnose-stock-art.py \
  --runtime-root .build/android-art-runtime \
  --stock-inputs .build/android-stock-inputs \
  --dex .build/android-art-dex/classes.dex \
  --qemu-user /path/to/static/qemu-aarch64 \
  --output .build/android-art-host-diagnostic
```

This creates fresh property areas, invokes the original `linkerconfig --target
/linkerconfig` as stock init does, and runs ART without `LD_LIBRARY_PATH`.
It saves commands, exit/timeout status, stdout/stderr, decoded and raw logd
messages, and generated-file hashes. **Do not add `--strict` to linkerconfig**
for this fixture: the image's absent optional `libdexfiled.so` provider causes
strict resolution to stop early while the program still exits zero. The
diagnostic rejects generator ERROR/FATAL logs as well as nonzero exits.
Java markers and normal exit produce `HOST_LINUX_PASS`, never a Scarlet pass.

Cloud verification on 2026-10-28 reached `HOST_LINUX_PASS` with QEMU `10.0.13`:
property initialization, stock linkerconfig and ART each exited zero, and Java
printed all three expected lines. The added inputs contain 72 hashed files and
12 bounded library aliases. The logs, commands, source identities and hashes
are retained in [the stock ART checkpoint](checkpoints/2026-10-28-cloud/stock-art/verification.json).
The host test suite passes 25 tests, including rejection of partial linkerconfig
output despite exit zero. Expected imageless-boot and missing tracefs/DEX
metadata diagnostics remain visible in the ART log.

The local Scarlet runner now accepts the same verified stock overlay:

```sh
python3 tools/android-compat/run-direct.py \
  --kernel projects/aarch64-limine-crosvm/bsp/target/aarch64-unknown-none-elf/release/scarlet \
  --runtime-root .build/android-compat/art-runtime \
  --stock-inputs .build/android-compat/stock-inputs \
  --art-dex .build/android-compat/dex/classes.dex \
  --output .build/android-compat/stock-art-run --accel hvf --timeout 180
```

The fixture gives this Android Environment a private writable `/dev` overlay
and binds TmpFS directly at `/dev/__properties__`. The real Bionic helper
initializes 216 stock values, then the original Android linkerconfig generates
its configuration on Scarlet. The entire generated ART configuration must
match the passing host reference before ART starts. The reference is checked
only; it is never installed as target policy. `LD_LIBRARY_PATH` is omitted.
Local datagram/logd collection is still unavailable, so exact configuration
comparison guards against partial output despite the generator's zero exit.
`evidence.py --art --stock` additionally requires the ordered bootstrap records.
`--linker-debug` enables Android linker diagnostics for startup investigations.

**Direct ART/Java now passes on Scarlet, with both outer QEMU HVF and TCG.**
The ordinary stock-input runs initialize 216 properties, generate the exact
reference linker configuration, print `SCARLET_ART_BEGIN`, `Scarlet ART: 42`
and `SCARLET_ART_OK`, then exit zero and emit `SCARLET_ANDROID_ART_OK`.
Neither successful run uses a diagnostic preload or a guest Linux kernel.
This is a core-only, image-less `-Xint` Java checkpoint; Android framework,
Activity/APK execution and graphics are still unverified.

The earlier [local checkpoint](checkpoints/2026-10-28-local/verification.json)
stalled after library constructors. Bionic's main-thread stack query needed
`getrlimit(RLIMIT_STACK)` and `/proc/self/stat` field 28. Native entry-stack
metadata resolved that path, exposing Bionic atexit-array growth, then ART's
condition-variable requeue and Signal Catcher's synchronous signal wait.
Those operations are now implemented. One intermediate diagnostic run printed
the Java markers while Signal Catcher was aborting and exited 127; it remains
classified as incomplete, not a successful Java completion.

The current kernel passes all 1,607 AArch64 tests and a RISC-V release check;
host checks pass 26 tests. Both HVF and TCG pass all eighteen expanded Bionic
cases. Logs, launch/input hashes, the diagnostic logger source and validation
excerpts are retained in [the ART/Java checkpoint](checkpoints/2026-10-28-art-java/verification.json).
Cloud's earlier Nix download restriction does not apply to this local validation.

### ABI composition and native RPC boundary

`kernel/src/abi/android/aarch64.rs` registers `android-aarch64` as an independent
ABI. It owns a `LinuxAarch64Abi` component and delegates common syscalls, ELF
loading, signals, clone and process lifecycle to that implementation. Android
device translations are supplied through `linux/extensions.rs`; the Linux
implementation has no dependency on the Android module. `AbiModule::base_abi`
allows exec to find compatible descriptor state inside the composition without
copying Android extensions into a plain Linux target.

Automatic detection requires a bounded, valid AArch64 ELF identification with
an Android NT_VERSION note or `/system/bin/linker64` (also the runtime APEX
linker path). A foreign interpreter takes precedence over a note. Unmarked
ELFs are not claimed by Android, even when the caller uses Android; select
`android-aarch64` explicitly with `Environment::spawn_with_abi`/`abi-run` for
those binaries. A generic Linux heuristic cannot tie with positive Android
identification. The Android filesystem view lives at `/systems/android-aarch64`
and must be registered in the execution Environment.

`kernel/src/ipc/rpc.rs` owns ABI-neutral endpoints, bounded request queues,
single-use reply capabilities and transferable object capabilities. Messages
carry up to 64 typed object references separately from serialized bytes. Each
object has a unique endpoint-local dispatch key; copying a key alone grants no
reference. Object references can be forwarded in requests or replies. They do
not keep the serving endpoint alive. Dropping a caller cancels its request;
closing an endpoint revokes object dispatch and completes outstanding calls.
Individual objects can also be revoked without closing sibling objects. Opaque
adapter leases travel with messages, and weak completion notifiers let callers
share an adapter's readiness source. A public native handle/syscall API is not
exposed yet.

`kernel/src/abi/android/binder/` translates the **64-bit** Binder UAPI.
Only the Android ABI recognizes `/dev/binder`, `/dev/hwbinder` and `/dev/vndbinder`
through the common Linux `openat` implementation.
Each device has an independent context manager **within each execution
environment**, even if two environments share a PID namespace. No-environment
opens return `ENODEV`; inherited descriptors cannot operate for another process
or after changing execution environment. Kernel tests exercise isolation by
attempting real adapter transactions across these boundaries.

The initial adapter supports protocol version 8, context-manager registration,
manual loopers, `BINDER_WRITE_READ`, synchronous and one-way transactions to handle 0 or a
transferred strong handle, replies, transaction completion, `BC_FREE_BUFFER`, thread exit and poll
waits with timeout/interruption through native RPC Wakers. Existing Linux epoll
readiness scans also observe Binder's level-triggered state. A read-only
receive VMA owns only its backing, so closing the file still completes pending
calls while the VMA remains mapped. Fork omits these process-specific mappings;
threads sharing an address space retain them. User copies and waits execute
without holding the adapter's IRQ lock.
Endpoint lifetime follows the open-file description: an inherited description
can delay closure even though a foreign process cannot operate it.

Flat strong/weak Binder objects and handles are translated through native RPC
capabilities. Handles retain identity when forwarded to a third process and
translate back to the original pointer/cookie when returned to their owner.
Receive buffers hold temporary references; `BC_INCREFS`/`BC_ACQUIRE` retain
explicit references beyond buffer release, and `BC_RELEASE`/`BC_DECREFS`
release them. Owner notifications and their DONE acknowledgements guard the
userspace object's lifetime. Completion waits for every locally exported
object's acquisition, including acquisitions begun by another thread. Local
payload delivery waits while acquisitions are pending. If that thread exits,
affected objects are revoked and acknowledged holds are released after leases
drain. An acquire delivered without its DONE cannot safely be undone; its
userspace hold may remain until process exit. `TF_CLEAR_BUF` is preserved and
freed receive slots are cleared.

Death subscriptions use native RPC capability watches. The watches retain
object identity without retaining its serving endpoint or the client's wait
source. `BC_REQUEST_DEATH_NOTIFICATION`, `BC_CLEAR_DEATH_NOTIFICATION` and
`BC_DEAD_BINDER_DONE` implement `BR_DEAD_BINDER` and
`BR_CLEAR_DEATH_NOTIFICATION_DONE`. Clearing queued/delivered death work waits
for its acknowledgement; clearing a live subscription cancels it. Failed
record copies remain retryable. Handle release cancels attached work, while
already detached clear work survives until delivery. Idle manual loopers
receive process death work; clear acknowledgements requested by a looper stay
on that thread, falling back to process work if it exits.

One-way calls complete at sender admission without creating a reply wait. Native
RPC serializes them per dispatch object until the receiver releases its delivery;
the Binder adapter retains that delivery through `BC_FREE_BUFFER`. Other objects
and synchronous calls can proceed while that notification is in service.
The queue bound includes both queued and delivered notifications. One-way
callbacks are also allowed while a thread handles a synchronous request.

Limits: 64 in-flight native calls/notifications per endpoint, 64 tracked threads per Binder
open, 1,024 local/retired nodes, 1,024 imported handles and 1,024 death records
(including pending clear acknowledgements), one incoming/outgoing
call per thread, at most 64 flat objects per message, and a
64 KiB–4 MiB receive VMA using 64 KiB slots. Partial unmap invalidates the arena.
The native message limit includes a 24-byte adapter header, aligned payload and
offsets; a byte-only payload can contain up to 65,512 bytes. Handle numbers are
not reused within an open file.
Fixed-address mmap and remapping an arena are rejected. The device aliases are
not a binderfs implementation and are not yet represented by normal VFS nodes.

**Not implemented:** FD/buffer-object transfer, weak-to-strong promotion,
nested synchronous calls, automatic thread-pool
growth, priority inheritance, security-context records and full SELinux policy.
Unsupported transaction features return errors. Stock libbinder serves a custom
`BBinder` probe through this adapter; stock servicemanager registration, lookup,
authorization, notifications and service death are now tested below.
The separate minimal ART/Java fixture passes; direct APK execution remains unverified.

The [object checkpoint](checkpoints/2026-10-29-binder-objects/verification.json)
records the three-process NDK UAPI probes in both static and dynamic Bionic,
the full 1,623-test kernel suite on HVF, and stock image-less ART `-Xint` Java
execution on the same release kernel. The host requires ordered object-transfer
and reference-release markers; the earlier byte-only probe cannot satisfy it.

The [service checkpoint](checkpoints/2026-10-28-binder-services/verification.json)
records the poll regression probes and native object transfer/lifetime tests.
The earlier unmodified SDK `servicemanager` diagnostic exited with status 127;
a separate diagnostic run redirecting Android logs to stderr identifies
`selinux_status_open(true)` as the failing check. It does not reach a usable
service manager. Stock libbinder is now tested separately below; SELinux
compatibility is now being built on [Shield](../../docs/security/shield.md).

`prepare-servicemanager.py` extracts the executable from the same pinned SDK
archive as `prepare-art.py` and checks its library dependencies against the
prepared runtime. `run-direct.py --program <executable> --runtime-root <runtime>
--stock-inputs <stock> ...` launches it for diagnosis. This mode records exit or
timeout and explicitly leaves service readiness untested; it cannot produce a
service PASS merely because a daemon stays alive or exits zero.

### Stock libbinder service probe

The [libbinder checkpoint](checkpoints/2026-10-29-stock-libbinder/verification.json)
runs unmodified SDK `libbinder.so`, `libutils.so`, `liblog.so` and platform
`libc++.so` on HVF. A custom C++ probe creates a context object in one process,
then publishes `BBinder` services to a separate client through real `Parcel`
transactions. It uses manual polling and no worker pool. It does not replace
servicemanager or exercise Android framework service registration.

Two creation/release cycles verify nonzero remote handles, interface tokens,
sender PID, integers and UTF-16 strings, return of a Binder reference to its
owner, null references, and `UNKNOWN_TRANSACTION` status replies. A client
`wp<IBinder>` keeps only the weak reference alive after the final strong
reference is dropped. The server verifies the actual `BBinder` destructor and
`RefBase` weak count: owner plus driver, then owner only after weak release.
The client also resolves `ProcessState::self` with `dladdr` to check that the
loaded implementation is `/system/lib64/libbinder.so`.

The [death-notification checkpoint](checkpoints/2026-10-29-binder-death/verification.json)
extends this probe with real `linkToDeath`/`unlinkToDeath` calls. After cancelling
one subscription and registering another, the server exits with a live service.
Only the active recipient receives a callback, exactly once; processing
clear completion restores the proxy's measured weak-reference count. Further
transactions and subscriptions return `DEAD_OBJECT`, and polling finds no
duplicate notification. This covers the death-notification prerequisite for
stock servicemanager's `addService`; its authorization adapter is described below.

The builder downloads AOSP Android 13 headers at fixed Git commits and verifies
their complete regular-file content maps. It links the stock platform C++ ABI
(`std::__1`/`libc++.so`) rather than the NDK C++ runtime. Source snapshots, header
pins, ELF dependencies and runtime library hashes are recorded in `build.json`.
The runner rejects stale builds and requires every ordered RPC/lifetime marker,
stock initialization, and successful exits from both processes. `--program`
retains its diagnostic-only meaning.

From `nix develop`, using the prepared runtime and stock overlay above:

```sh
python3 tools/android-compat/build-libbinder-probe.py \
  --ndk "$ANDROID_NDK_HOME" \
  --runtime-root .build/android-compat/art-runtime \
  --output .build/android-compat/libbinder-probe
python3 tools/android-compat/run-direct.py \
  --kernel projects/aarch64-limine-crosvm/bsp/target/aarch64-unknown-none-elf/release/scarlet \
  --libbinder-probe .build/android-compat/libbinder-probe \
  --runtime-root .build/android-compat/art-runtime \
  --stock-inputs .build/android-compat/stock-inputs \
  --output .build/android-compat/libbinder-run --accel hvf
```

### Kernel fixes exercised

- Store bounded generic xattrs on TmpFS inodes and translate Linux's twelve
  xattr calls. Attributes retain inode identity through rename/unlink, and
  their bytes count against TmpFS quota. `security.selinux` is opaque metadata;
  SELinux access enforcement is not implemented. Other filesystems return
  `EOPNOTSUPP` and read-only mutations return `EROFS`.
- Relocate same-size private normal-memory ranges in the native VM and expose
  the operation through `mremap(MREMAP_MAYMOVE | MREMAP_FIXED)`. Resident pages,
  lazy owners, permissions and fork/COW page indices survive the move. Source
  and destination PTEs are invalidated before displaced backing is reclaimed;
  fault publication revalidates its VMA snapshot. Shared/file/device
  owners, mixed source origins/protections, and a partial move whose rebased
  origin would underflow remain unsupported.
- Grow private anonymous mappings in place when the following range is free,
  or select and move into a free range when `MREMAP_MAYMOVE` is permitted.
  Anonymous provenance survives faults and fork. The extension uses fresh zero
  backing, including after shrink/regrowth; existing backing and COW indices
  are preserved, and rejected growth retains the source. Fixed growth and
  `MREMAP_DONTUNMAP` remain unsupported.
- Keep the initial SP and fixed stack reservation in the native address space;
  fork copies it, `CLONE_VM` shares it and committed exec replaces it. Linux
  exposes the reservation through `getrlimit`/read-only `prlimit64` and stat
  field 28. Only `RLIMIT_STACK` and `RLIMIT_NOFILE` queries are backed; other
  limits and changes return an error. Stack auto-growth and full proc accounting
  are not implemented; unavailable accounting fields are zero.
- Implement Linux `FUTEX_REQUEUE` and `FUTEX_CMP_REQUEUE` on the existing Linux
  wait queues, preserving bitsets and tracking the current queue for timeout,
  interruption and task-exit cleanup.
- Implement `rt_sigtimedwait` through existing Linux pending state and Scarlet's
  interruptible Waker, with relative timeout, pending-signal consumption and
  output validation. Like signalfd, this uses coalesced pending bits and reports
  `SI_KERNEL` for unavailable sender metadata. Native signal records, realtime
  queues and process/thread routing are tracked in
  [discussion #613](https://github.com/petitstrawberry/Scarlet/issues/613).
- Classify Ext2 inline symlinks by data-block allocation (excluding an external
  attribute block), rather than target length. This reads e2fsprogs' block-backed
  60-byte links and reclaims their blocks while retaining support for older
  Scarlet inline links. New 60-byte links use data blocks, matching e2fsprogs.

- Provide `AT_SECURE=0` for executions with no privilege transition; Bionic
  treats a missing entry as a fatal startup error.
- Reject unsupported tagged-pointer syscall controls with `EINVAL` instead of
  falsely accepting Bionic's feature probe. MTE remains unadvertised.
- Enable AArch64 TBI0 for userspace memory accesses and normalize user data-fault
  addresses before page lookup. Scudo uses a fixed internal header tag even
  when heap tagging is disabled. This is separate from accepting tagged syscall
  pointers; see [Linux's tagged-address rules](https://docs.kernel.org/arch/arm64/tagged-pointers.html).
- Follow `/proc/{self,thread-self,pid}/exe` for stat/statx using the executable
  owner's filesystem view; no-follow calls still describe the symlink. The
  existing path-based executable identity does not retain deleted/replaced
  executable inodes.
- Open the bootstrap console read/write before mapping standard handles.
  Passing a read-only handle as stdout caused Linux writev to return EBADF and
  concealed both Bionic output and crosvm diagnostics.

## VM APK checkpoint

Validation on 2026-10-28: APK build/signature verification and profile preparation
passed. The 1,800-second TCG run reached SurfaceFlinger (ANGLE/SwiftShader GLES
3.1) and SystemServer bootstrap services, but timed out before
`sys.boot_completed=1`. APK execution and PNG capture remain **unverified**;
no screenshot was produced. A second 1,800-second run with the SHV timer/re-entry
fixes and preserved Android userdata also timed out, with Activity service
registration still pending, property-socket timeouts and Linux workqueue-stall
reports. This does not establish whether host emulation load or a remaining
SHV defect dominates the stall. The revised kernel passes the small SHV fixture;
that result does not certify Android framework boot.

The fixture APK needs no Gradle or downloaded Java dependencies. It draws a
small Canvas view and reports lifecycle/input events with a unique per-run
nonce. Use a local Android SDK and JDK (`JAVA_HOME`):

```sh
python3 tools/android-compat/build-apk.py --sdk "$ANDROID_HOME" \
  --build-tools 37.0.0 --platform android-32 --output .build/android-compat/apk
python3 projects/aarch64-limine-crosvm/tools/prepare-cuttlefish.py \
  --image-zip /path/to/aosp_cf_arm64_only_phone-img-16373615.zip \
  --base-staging projects/aarch64-limine-crosvm/.scarlet/base-gpu/staging \
  --smoke-apk .build/android-compat/apk/scarlet-compat.apk \
  --kernel-log --output projects/aarch64-limine-crosvm/.scarlet/android-apk
python3 projects/aarch64-limine-crosvm/tools/run.py \
  --kernel projects/aarch64-limine-crosvm/bsp/target/aarch64-unknown-none-elf/release/scarlet \
  --staging projects/aarch64-limine-crosvm/.scarlet/android-apk/staging \
  --output projects/aarch64-limine-crosvm/.scarlet/android-apk-run \
  --timeout 1800 --success-marker SCARLET_ANDROID_APK_OK
python3 tools/android-compat/evidence.py \
  projects/aarch64-limine-crosvm/.scarlet/android-apk-run/serial.log \
  --apk --output .build/android-compat/apk-evidence
```

Build the GPU-enabled base staging tree as described in the crosvm README.
The APK builder signs with a disposable local test key and verifies the
signature. Preparation appends a read-only payload disk as `vdc`, preserving
`vda` (system) and `vdb` (metadata/userdata/misc). It never installs into a
connected phone or an existing user Android instance.

Only after `sys.boot_completed=1` does the guest install the APK, launch its
Activity, check the matching resume event, inject a tap, check the matching
touch event, and capture the screen. The host independently validates the PNG
signature, chunk CRCs and bounded decompressed pixel data. Inspect the resulting
frame for actual visible contents; a valid PNG alone does not prove its subject.
APK execution, guest frame capture and presentation in an SWS window are distinct
milestones. SWS presentation is not implemented by this fixture.

`--kernel-log` keeps a native parent forwarding Scarlet's kernel ring, useful
for failures before crosvm initializes logging. It is a diagnostic launcher,
not a process supervisor; the host runner owns timeout and shutdown.

## Host checks

```sh
python3 -m unittest discover -s tools/android-compat -v
python3 -m unittest discover -s projects/aarch64-limine-crosvm/tests -v
```

The evidence tests reject zero-exit runs with missing child output, boot markers
without captures, missing APK input, corrupt PNGs and interleaved console logs.
Kernel regression tests additionally cover auxv, unsupported prctl requests
and tagged fault-address normalization, RPC cancellation/lifetime/bounds,
Binder environment/device isolation, receive-buffer validation and Android ELF
identification (including malformed/truncated inputs). The final AArch64 suite
passes 1,623 tests on HVF; the RISC-V kernel library also compiles.

### Shield and stock libselinux

[Shield](../../docs/security/shield.md) supplies native policies and process
labels independently of Environment/View/Namespace. The fixture explicitly
creates a policy in userspace and mounts the Android query adapter; ordinary
Android environments do not automatically acquire a policy.

The [Shield checkpoint](checkpoints/2026-10-29-shield/verification.json) exercises
unmodified Android 13 `libselinux.so`: real allowed and denied `service_manager`
queries, unknown-label/class/permission rejection, refusal of permissive and
process-label writes, an immutable mapped status page, and label inheritance
through fork/exec. Query-only policy handles cannot assign a different label.
That initial checkpoint did not certify stock servicemanager registration/lookup
or full SELinux policy enforcement. The service integration below now covers
registration and lookup; full SELinux policy enforcement remains unsupported.

From `nix develop`, using the same NDK, prepared runtime and stock overlay:

```sh
python3 tools/android-compat/build-selinux-probe.py \
  --ndk "$ANDROID_NDK_HOME" --runtime-root .build/android-compat/art-runtime \
  --output .build/android-compat/selinux-probe
python3 tools/android-compat/run-direct.py \
  --kernel path/to/scarlet.elf --selinux-probe .build/android-compat/selinux-probe \
  --runtime-root .build/android-compat/art-runtime \
  --stock-inputs .build/android-compat/stock-inputs \
  --output .build/android-compat/shield-run --accel hvf
```

### Stock servicemanager integration

The userspace fixture now launches the unmodified Android 13 `servicemanager`
with separate Shield identities for the manager, publisher, client and denied
peer. A small explicit service-context file maps the test service names into
that immutable policy. Environment/View/Namespace still have their existing
roles; no Android process manager is embedded in the ABI module.

The [service checkpoint](checkpoints/2026-10-29-services/verification.json)
requires the real IServiceManager path to register a service, look it up from
another process, call it, and list it. A differently labeled process is denied
add/find/list. Stock registration notifications exercise a one-way callback
while servicemanager handles a synchronous request. Three ordered one-way
calls then reach the service. After the publisher exits, servicemanager's death
recipient removes the entry, and the retained proxy returns `DEAD_OBJECT`.
The fixture kills and reaps its own manager only after all peers succeed.

All runtime checks use HVF. This checkpoint passes 1,643 kernel tests, 18 Bionic
cases, 38 host checks, the stock libbinder lifetime/death regression and stock
libselinux authorization checks; the RISC-V library compiles. No patched runtime
DSO or logging preload is used by the service integration. This is not a full
Android boot, Zygote/framework startup, or APK execution claim.

From `nix develop`, after the documented runtime/stock/servicemanager preparation:

```sh
python3 tools/android-compat/build-libbinder-probe.py --servicemanager \
  --ndk "$ANDROID_NDK_HOME" --runtime-root path/to/art-runtime \
  --output .build/android-compat/service-probe
python3 tools/android-compat/run-direct.py --kernel path/to/scarlet.elf \
  --service-probe .build/android-compat/service-probe \
  --service-inputs path/to/verified-service-inputs \
  --runtime-root path/to/art-runtime --stock-inputs path/to/stock-inputs \
  --output .build/android-compat/service-run --accel hvf
```


### Stock app_process framework/JNI checkpoint

The [2026-10-29 framework checkpoint](checkpoints/2026-10-29-framework/verification.json)
passes on HVF with stock `app_process64`, `AndroidRuntime`/`RuntimeInit`, platform
and APEX boot JARs, and the tethering JNI library used by `TrafficStats`.
`ScarletFrameworkHello` checks native process identity, a Bundle/Parcel round
trip, local Java Binder dispatch, and a HandlerThread's immediate/delayed
callbacks, wakeup and joined shutdown. It returns from `main`; the process exits
zero without `System.exit`, diagnostic preloads or changes to stock Android DSOs.

This exposed a Binder close bug: a pool thread retained the driver file while
blocked in `BINDER_WRITE_READ`, so final object destruction could not wake it.
The Android adapter now implements per-descriptor flush, returning `BR_NOOP` to
registered threads on close or dup3 replacement while preserving other open
references. The Linux composition interface supplies the close hook. Failed
usercopy preserves the notification. Native tests and static/dynamic Bionic
probes also cover the behavior.

Prepare new stock inputs with `prepare-stock-art.py` to include Android init's
image-derived CPU ABI properties, then run inside `nix develop`:

```sh
python3 tools/android-compat/prepare-framework.py \
  --sdk-image "$SDK_IMAGE" --runtime-root "$ART_RUNTIME" \
  --stock-inputs "$STOCK_INPUTS" --output "$FRAMEWORK_INPUTS"
python3 tools/android-compat/build-dex.py \
  --build-tools "$ANDROID_SDK_ROOT/build-tools/37.0.0" \
  --framework-sdk "$ANDROID_SDK_ROOT/platforms/android-32/android.jar" \
  --output "$FRAMEWORK_PROBE"
python3 tools/android-compat/run-direct.py \
  --kernel "$SCARLET_KERNEL" --framework-probe "$FRAMEWORK_PROBE" \
  --framework-inputs "$FRAMEWORK_INPUTS" --runtime-root "$ART_RUNTIME" \
  --stock-inputs "$STOCK_INPUTS" --accel hvf --output "$FRAMEWORK_RUN"
```

The extractor preserves classpath order from the pinned image's protobuf
fragments (ART, system, then sorted remaining APEXes) and checks release API
bounds. Manifests bind every staged JAR, executable, JNI library and probe DEX.
The runner records its explicit `dalvik.vm.usejit=false` fixture override;
AndroidRuntime otherwise appends the stock `true` value after the command-line
`-Xint` options. Source image properties remain unchanged. Console evidence uses
the inherited stdout descriptor because application-mode RuntimeInit redirects
`System.out` to logd.

This proves a framework Java process and JNI APIs, not Android system boot,
zygote/system_server, cross-process Java Binder, an Activity or an APK. The
checkpoint includes the previous timeout with completed Java markers, the thread
dump identifying the waiting Binder pool thread, and passing regressions: 1,644
kernel tests, 18 Bionic cases, stock service/libbinder/libselinux checks, 43 host
tests and the RISC-V release library check.


### Java Binder services checkpoint

`checkpoints/2026-10-29-java-binder/verification.json` extends the framework
checkpoint to three separate Java processes and stock servicemanager. The
publisher registers a Java Binder, the client calls it through JNI, and Shield
rejects the denied peer's lookup and registration. Four ordered one-way calls
transfer Java callback objects and return Bundle payloads. Owner shutdown
triggers a Java DeathRecipient, DeadObjectException, and stock manager cleanup;
all three Java main methods return normally and are reaped with status zero.

```sh
python3 tools/android-compat/run-direct.py \
  --kernel "$SCARLET_KERNEL" --framework-services "$FRAMEWORK_PROBE" \
  --framework-inputs "$FRAMEWORK_INPUTS" --service-inputs "$SERVICE_INPUTS" \
  --runtime-root "$ART_RUNTIME" --stock-inputs "$STOCK_INPUTS" \
  --accel hvf --output "$JAVA_SERVICES_RUN"
```

Rebuild the framework probe with `build-dex.py --framework-sdk` before using
this mode. Source/DEX, framework, manager, service-context and native launcher
hashes are bound into the result. The kernel transports the framework's AppOps
collection flag instead of rejecting Java calls with EOPNOTSUPP; collection
remains the receiving framework's job. This checkpoint does not establish an
AppOps service, Android system boot or APK/Activity launch. HVF validation also
passes 1,645 kernel tests and the C++ service lifecycle regression, alongside
45 host tests and the RISC-V release library check.


### APK archive and payload checkpoint

`checkpoints/2026-10-29-apk-runtime/verification.json` records stock Android 13
APK basic parsing, full signature verification, resources and DEX loading on
Scarlet. The signed fixture uses APK Signature Scheme v3. Both the guest verifier
and host `apksigner` accept it and reject a copy with one changed signed asset
byte. Stock resource JNI reads the compiled manifest, framework and application
resources, and assets. A separate DexClassLoader loads the APK's RuntimePayload,
which uses Resources and Bundle/Parcel, and links its Activity subclass. Java
returns normally and app_process exits zero.

```sh
python3 tools/android-compat/build-apk.py --sdk "$ANDROID_SDK_ROOT" \
  --runtime-probe --output "$APK_PROBE"
python3 tools/android-compat/run-direct.py \
  --kernel "$SCARLET_KERNEL" --apk-probe "$APK_PROBE" \
  --framework-inputs "$FRAMEWORK_INPUTS" --runtime-root "$ART_RUNTIME" \
  --stock-inputs "$STOCK_INPUTS" --accel hvf --output "$APK_RUNTIME_RUN"
```

Regenerate framework inputs with `prepare-framework.py` to include the image's
framework-res.apk and Conscrypt JNI libraries. The probe uses stock archive
inspection APIs, including `forParsingWithoutPlatformCompat` and the full
`ApkSignatureVerifier.verify` path. Full package parsing requires Android's
PermissionManager service, as retained in the diagnostic logs. Activity launch,
installation, application identity isolation and graphics remain untested.
The result explicitly reports `apk_execution: payload DEX only` and
`activity_launch: not tested`. Host validation covers 48 tests.


### Zygote specialization and conditional labels

Shield policies can delegate an exact source-to-target label transition for a
native principal. All three principal IDs must match; merely changing the active
ID does not qualify. Android `setcon` uses this grant through an owner-bound
`/proc/self/attr/current` descriptor. Inherited descriptors cannot relabel another
process. Policies remain immutable, and ordinary policy query handles do not
gain the ability to assign arbitrary labels.

Build `build-selinux-probe.py --transitions` and run it with
`run-direct.py --label-transition-probe` to check stock libselinux transitions for
two app identities, rejected cross-identity/reverse transitions, inherited file
descriptors, and fork/exec retention. Regenerate framework inputs to include the
stock `plat_seapp_contexts` file.

```sh
python3 tools/android-compat/build-zygote-probe.py --child \
  --ndk "$ANDROID_NDK_ROOT" --build-tools "$ANDROID_SDK_ROOT/build-tools/37.0.0" \
  --framework-sdk "$ANDROID_SDK_ROOT/platforms/android-32/android.jar" \
  --framework-inputs "$FRAMEWORK_INPUTS" --runtime-root "$ART_RUNTIME" \
  --stock-inputs "$STOCK_INPUTS" --output "$ZYGOTE_CHILD_PROBE"
python3 tools/android-compat/run-direct.py \
  --kernel "$SCARLET_KERNEL" --zygote-probe "$ZYGOTE_CHILD_PROBE" \
  --framework-inputs "$FRAMEWORK_INPUTS" --runtime-root "$ART_RUNTIME" \
  --stock-inputs "$STOCK_INPUTS" --accel hvf --output "$ZYGOTE_CHILD_RUN"
```

The userspace fixture applies an enforcing Zygote policy, requests UID/GID 10001
through the stock control socket, and checks the resulting app label selected
by stock libselinux. A fixture Java main added to the boot classpath returns a
proof through a separate socket. The supervisor matches it against the kernel's
`SO_PEERCRED`, the PID returned by Zygote, and the live process label. It then
acknowledges the child and observes socket closure. The supervisor reaps its own
Zygote; the grandchild's exit status is not observed. There is no runtime preload.

This validates Zygote child specialization and Java entry, not Activity launch,
package installation, system_server boot, graphics, or complete app confinement.
Category suffixes are exact opaque labels; Shield does not implement SELinux's
MLS lattice. Cgroup accounting and general VFS ownership/DAC remain separate work.

### Profile-selected SystemSuspend host service

The flattened boot profile selects framework AIDL declarations from the verified
SDK image through `vintf.framework`; it publishes no unimplemented device HALs.
`host_services: ["system_suspend"]` additionally stages the stock SystemSuspend
executable and its native dependencies. The manager generates an init declaration
from the checked stock RC, omitting `BLOCK_SUSPEND`, which Scarlet does not expose.
Source and generated hashes are recorded in `boot-profile.json`; staging rejects a
missing provider or a stale declaration.

The stock daemon already supports host-managed power: absent suspend sysfs leaves
its suspend thread blocked while Binder wake-lock bookkeeping remains active.
Scarlet does not synthesize suspend sysfs or delegate machine suspend to Android.
The profile grants the daemon its own exec label and service registration rights,
and gives system_server explicit lookup rights. These settings do not establish
PowerManager readiness; the stock-init execution logs record the reached stage.
