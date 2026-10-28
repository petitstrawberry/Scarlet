# Android compatibility: two execution paths

Both paths have runnable, isolated fixtures. They answer different questions;
neither fixture certifies general APK compatibility.

| | Android in a VM | Direct Android userspace |
|---|---|---|
| Execution | APK → Android framework/ART → guest Linux → crosvm → Scarlet | Android ELF → Bionic/linker → Scarlet Android ABI (composes Linux ABI) |
| Reused Android facilities | Guest Binder, Android init, framework and HALs | Real Bionic/linker and an initial Binder UAPI adapter |
| Main work ahead | Finish framework boot, then connect display/input to SWS | Binder object references, ART/JNI, framework services and graphics integration |
| Current rendering | Cuttlefish ANGLE/SwiftShader, headless virtio-gpu stub | No Android graphics stack yet |
| Fixture | [Cuttlefish project](../../projects/aarch64-limine-crosvm/README.md) | This directory |

The direct fixture uses Android 13/API 33 while the pinned VM uses Android 17.
These are compatibility checkpoints, not a controlled performance comparison.
Continue the VM track toward a real APK while the direct track advances through
Bionic → Binder client/server → ART/JNI → a minimal framework application.

## Direct Bionic execution

Verified on 2026-10-28: **fourteen cases pass** with `--binder` on Scarlet/AArch64 with outer QEMU
HVF and TCG, Android NDK r25b targeting API 33, and an Android 13 ARM64 runtime APEX
from the locally installed SDK image. No guest Linux kernel is involved.

Each case runs in a fresh process and must produce both child `BEGIN`/`OK`
records and exit status zero. The host also requires the final suite marker.

| Case | Static | Dynamic | Checks |
|---|---|---|---|
| libc | PASS | PASS | Constructors, TLS, auxv, malloc, proc-exe stat/lstat/statx, tagged memory accesses |
| thread | PASS | PASS | pthread create/join, TLS isolation, mutex and shared address space |
| shared | PASS | PASS | memfd and coherent MAP_SHARED mappings |
| signal | PASS | PASS | SIGUSR1 registration, delivery and handler return |
| dynamic | — | PASS | dlopen/dlsym/dlclose, DSO constructor and native ELF TLS |
| exec | PASS | PASS | Sparse FD numbers, file flags, CLOEXEC and Android device dispatch survive exec |
| binder | PASS | PASS | Cross-process request/reply, read-only receive pages, buffer release, pending-call peer death and fork isolation |

An additional static probe explicitly selects `linux-aarch64` and verifies that
none of the three Binder aliases is exposed. This is the fourteenth case.

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
No custom namespace config was installed. **Owner consultation is pending**
before choosing a fixture-specific namespace/access policy versus reproducing
stock Android property/APEX state and generated namespace configuration.
The current ART runner is a startup diagnostic, not a passing Java fixture.

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

`kernel/src/ipc/rpc.rs` owns ABI-neutral endpoints, bounded request queues and
single-use reply capabilities. Dropping a caller cancels its request; closing
an endpoint or dropping an unanswered request completes pending calls with an
error. These are internal kernel objects; a public native handle/syscall API
and transferable object references are not exposed yet.

`kernel/src/abi/android/binder/` translates the **64-bit** Binder UAPI.
Only the Android ABI recognizes `/dev/binder`, `/dev/hwbinder` and `/dev/vndbinder`
through the common Linux `openat` implementation.
Each device has an independent context manager **within each execution
environment**, even if two environments share a PID namespace. No-environment
opens return `ENODEV`; inherited descriptors cannot operate for another process
or after changing execution environment. Kernel tests exercise isolation by
attempting real adapter transactions across these boundaries.

The initial adapter supports protocol version 8, context-manager registration,
manual loopers, `BINDER_WRITE_READ`, byte-only synchronous transactions to handle
0, replies, transaction completion, `BC_FREE_BUFFER`, and thread exit. A read-only
receive VMA owns only its backing, so closing the file still completes pending
calls while the VMA remains mapped. Fork omits these process-specific mappings;
threads sharing an address space retain them. User copies and waits execute
without holding the adapter's IRQ lock.

Limits: 64 in-flight native calls per endpoint, 64 tracked threads per Binder
open, one incoming/outgoing call per thread, payloads up to 65,524 bytes, and a
64 KiB–4 MiB receive VMA using 64 KiB slots. Partial unmap invalidates the arena.
Fixed-address mmap and remapping an arena are rejected. The device aliases are
not a binderfs implementation and are not yet represented by normal VFS nodes.

**Not implemented:** object/FD transfer, strong/weak reference accounting,
general death subscriptions, one-way and nested calls, automatic thread-pool
growth, poll-based loopers, security-context records and SELinux policy.
Unsupported transaction features return errors. This checkpoint does not yet
run unmodified libbinder services, servicemanager, ART, or a direct APK.

### Kernel fixes exercised

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
passes 1,587 tests; the RISC-V kernel library also compiles.
