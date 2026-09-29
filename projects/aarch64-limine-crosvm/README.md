# AArch64 crosvm project

This dedicated project owns the crosvm, Linux and Android bring-up fixtures,
native init, hypervisor-enabled BSP and QEMU runner. It builds the kernel from
this checkout via `scarlet.toml`; it does not extend the repository's general
loader tools. Build outputs, selected guest profiles, images and logs live in
the project's ignored `.scarlet/` directory (Cargo's BSP output is under
`bsp/target/`). `fixtures/` contains guest sources and integration probes;
`tools/` contains only this project's preparation and execution tools.

The base fixture builds **unmodified upstream crosvm** at
`88c1385ccb8cb31e8e4d683f580c37057cfc4b48`, runs its Linux/glibc binary on
Scarlet, and executes a small AArch64 guest through Scarlet's KVM compatibility
layer. PASS requires the guest's `SCARLET_CROSVM_GUEST_OK` serial output.

The fixture uses one vCPU, 128 MiB of guest RAM, and crosvm's
`--no-default-features --features default-no-sandbox` build. This establishes
guest entry, HVC/SMC return-state checks and MMIO serial output. The separate
Linux fixture below verifies OS boot and virtio-block I/O. Neither fixture
establishes multivCPU operation, graphics or sandbox compatibility. The guest requests
PSCI shutdown after printing, but the runner stops QEMU at the success marker
and does not validate shutdown completion.

## Run

Use the Scarlet development environment (`nix develop`) with Docker running.
Required host tools: Python 3, Git, Docker with Linux/arm64 support, the Scarlet
Rust toolchain, Clang/LLD, `mkfs.ext2`, `cargo-scarlet-plugin-limine`, and
`qemu-system-aarch64`. Set `SCARLET_EFI_CODE_ARM64_EL2` and
`SCARLET_EFI_VARS_ARM64_EL2` to AArch64 UEFI code and a variable-store template
(the non-EL2 variable names are also accepted).

From the repository root:

```sh
python3 projects/aarch64-limine-crosvm/tools/build.py
cargo scarlet image --release --project projects/aarch64-limine-crosvm
cargo scarlet run --release --project projects/aarch64-limine-crosvm --no-image
```

`build.py`, `prepare-os.py` and `prepare-android.py` select their completed
profile through `.scarlet/active`. The normal image command uses its minimal
boot tree for initramfs and its staging tree for the ext2 root. Rebuild images
after selecting a different profile. The runner consumes these project images;
it does not rebuild a second set of boot disks.

For an isolated diagnostic run, `tools/run.py --kernel <ELF> --staging <tree>
--output <directory>` can prepare a separate image pair. Supply
`--gdb-socket /tmp/scarlet-crosvm-gdb.sock` there to debug. Kernel-only builds use
`cargo scarlet build --release --project projects/aarch64-limine-crosvm`; the release ELF is
`bsp/target/aarch64-unknown-none-elf/release/scarlet` under this project.

`build.py --source /path/to/crosvm` reuses a clean checkout at the pinned
revision, with its minijail submodule initialized. Otherwise it creates a new
checkout under the output directory. The first Docker/Rust build requires
network access and can take several minutes. Docker builds a local image named
`scarlet-crosvm-builder`; crosvm is compiled with `--locked`. The source revision
is pinned, but Ubuntu packages are not a bit-for-bit toolchain snapshot.

The native init mounts a private ext2 root disk and executes crosvm in Scarlet's
Linux AArch64 environment. ext2 supplies mmap support for glibc and the guest
image. QEMU uses TCG with EL2 virtualization enabled and a private firmware
variable store by default. Select HVF explicitly on an Apple Silicon host
with Hypervisor.framework EL2 support and QEMU 11.1:

```sh
SCARLET_QEMU_ACCEL=hvf SCARLET_CROSVM_SUCCESS_MARKER=SCARLET_CROSVM_LINUX_BLOCK_OK \
  cargo scarlet run --release --project projects/aarch64-limine-crosvm --no-image
```

This example assumes the Linux block-check profile has been prepared and its
images built. The private runner also accepts `--accel hvf`. It selects
`-cpu host`, prefers the HVF firmware pair, and keeps `virtualization=on` and
GICv3. Failure to initialize HVF is reported without a TCG fallback.
The release Linux block-check image passes under TCG. Nested HVF execution
remains unverified on the tested host; an EL2-capable firmware file alone does
not make EL2 available through Hypervisor.framework. Treat the HVF command as
a bring-up option, not a verified accelerated configuration.
Host `/dev/kvm` is not required; normal boot bundles and root
disk images are not modified.

Outputs include `serial.log`, `result.json`, `commands.json`, boot/root disk
images, and image preparation logs. For debugging, add
`--gdb-socket /tmp/scarlet-crosvm-gdb.sock --timeout 300`. Non-test kernel
diagnostics may be in Scarlet's kernel log ring rather than the serial console.

## Linux guest

Build the pinned Linux 6.12 source and a static AArch64 BusyBox in the same
Docker builder, then prepare an isolated fixture:

```sh
mkdir -p projects/aarch64-limine-crosvm/.scarlet/linux/guest
docker run --rm --platform linux/arm64 \
  -v "$PWD/projects/aarch64-limine-crosvm/.scarlet/linux/guest:/export" \
  -v "$PWD/projects/aarch64-limine-crosvm/tools/build-linux-guest.sh:/build.sh:ro" \
  scarlet-crosvm-builder sh /build.sh
python3 projects/aarch64-limine-crosvm/tools/prepare-os.py \
  --output projects/aarch64-limine-crosvm/.scarlet/linux-verified \
  --image projects/aarch64-limine-crosvm/.scarlet/linux/guest/Image \
  --busybox projects/aarch64-limine-crosvm/.scarlet/linux/guest/busybox --block-check
cargo scarlet image --release --project projects/aarch64-limine-crosvm
SCARLET_CROSVM_SUCCESS_MARKER=SCARLET_CROSVM_LINUX_BLOCK_OK \
  cargo scarlet run --release --project projects/aarch64-limine-crosvm --no-image
```

`build-linux-guest.sh` verifies the kernel tarball SHA-256 and records the
configuration, source checksum and installed package versions. It uses Ubuntu's
BusyBox package; this is not a fully pinned userspace build. Linux source is GPL-2.0;
BusyBox is GPL-2.0. Downloaded sources and binaries remain local build artifacts.

The initramfs checks a timer delay, fork/wait and file I/O. With `--block-check`
it mounts a private ext2 disk through Linux's ext4 driver, reads a known file,
writes a file, syncs, unmounts, remounts and verifies the write. PASS stops the
outer QEMU at the marker; it does not test power-loss durability or VM shutdown.
Without `--block-check`, select `SCARLET_CROSVM_LINUX_OK`. After either marker,
the init script opens a BusyBox shell if the VM is left running.

The local experimental bundle also contains these assets at
`fs/systems/linux-aarch64/usr/share/crosvm/linux/`. With an updated hypervisor
kernel and experimental bundle in the full image, run from Scarlet's shell
(one line):

```sh
abi-run linux-aarch64 /usr/bin/crosvm --no-syslog run --disable-sandbox --no-pmu --no-rng --no-usb --cpus 1 --mem 512 --serial type=stdout,hardware=serial,num=1,console=true,stdin=true --initrd /usr/share/crosvm/linux/initramfs.cpio --params "earlycon=uart8250,mmio,0x3f8 console=ttyS0 rdinit=/init nokaslr" --block path=/usr/share/crosvm/linux/data.ext2,ro=false,lock=false /usr/share/crosvm/linux/Image
```

This enters the Linux shell; use `mount -t ext4 /dev/vda /mnt` to access the
data disk. The automated fixture checks serial output, not interactive keyboard
delivery in the full desktop image. A separate TCG serial check used the actual
packaged crosvm/private runtime, entered `uname -r`, mounted/read the disk, and
requested `poweroff -f`. It observed the Linux 6.12 response, file contents,
KVM shutdown event and crosvm's `exiting with success` log. crosvm also logged a
closed VcpuControl channel during shutdown; host process exit status was not
captured by the exec-only launcher. The supplied crosvm ELF uses its private
glibc runtime under `/usr/lib/crosvm`; the surrounding Linux view may use musl.
Already-running images need rebuilding/rebooting to pick up kernel fixes and
new guest files.

The installed `run.sh` is a convenience copy of `fixtures/linux-run.sh` with the same
arguments. In Scarlet's shell:

```sh
abi-run linux-aarch64 /bin/sh /usr/share/crosvm/linux/run.sh
```

## Android bring-up

The current fixture uses the official Android API 35 ARM64 default emulator
image, revision 02, from the [Android SDK system-image index](https://dl.google.com/android/repository/sys-img/android/sys-img2-1.xml).
It is a ranchu emulator image, not a Cuttlefish image. Download
[arm64-v8a-35_r02.zip](https://dl.google.com/android/repository/sys-img/android/arm64-v8a-35_r02.zip)
and retain its bundled notices. Preparation verifies the published SHA-1
`2026a06409db630b56711afdbffb457c1dbaed49` before reading the archive.

```sh
python3 projects/aarch64-limine-crosvm/tools/prepare-android.py \
  --sdk-zip /path/to/arm64-v8a-35_r02.zip --output projects/aarch64-limine-crosvm/.scarlet/android
cargo scarlet image --release --project projects/aarch64-limine-crosvm
SCARLET_CROSVM_TIMEOUT=900 SCARLET_CROSVM_SUCCESS_MARKER=SCARLET_ANDROID_BOOT_OK \
  cargo scarlet run --release --project projects/aarch64-limine-crosvm --no-image
```

Preparation needs Python `lz4`, `mkfs.ext4` and `debugfs`. Use a new output directory;
the script refuses to recreate existing Android writable disks. It decompresses
the kernel and concatenated legacy-LZ4 ramdisk, adjusts the metadata/data device
paths in both the first-stage ramdisk and vendor's second-stage fstab for the
four virtio disks (`/metadata` on `vdc`, `/data` on `vdd`), creates private writable
metadata/userdata disks, and attaches the system/vendor GPT images read-only.
The vendor fstab is patched in both `vendor.img` and the logical vendor inside
`system.img`'s super partition. Only extracted copies are patched; fstab size,
inode metadata and SELinux labels are preserved. Writable ext4 disks disable `orphan_file` for
compatibility with Android's bundled e2fsprogs 1.46.6. Existing profiles need
preparing again in a new directory, followed by image generation. The boot-device
property names crosvm's `10000.pci` controller so Android can create partition
symlinks. Kernel/console logging uses virtio-console (`hvc0`), with MMIO UART early
output. A second virtio-console (`hvc1`) carries ranchu's `goldfish-logcat` output
to the same host stdout/`serial.log`; it is not the kernel console and has no stdin.
`androidboot.logcat=*:E` supplies the required `ro.boot.logcat` service argument;
without it, Android init aborts argument expansion before executing logcat.
Logcat forwards error/fatal messages from the main, system and crash buffers;
kernel messages already arrive via `hvc0` and are not replayed on `hvc1`.
The kernel uses `loglevel=4` without
`ignore_loglevel`, keeping boot-info and permissive audit noise off the console
so crash diagnostics are not delayed behind the boot log replay. SELinux remains
permissive; this changes console verbosity, not access policy.
Android is configured with 2 GiB RAM. The outer Scarlet runner defaults to 8 GiB; override it with
`SCARLET_QEMU_MEMORY` or `run.py --memory`.

The fixture disables the Ranchu sensors sub-HAL by leaving its `hals.conf`
empty: no sensor host transport is attached, so the multi-HAL should report zero
sensors instead of aborting in the Ranchu library constructor. Use
`--ranchu-sensors` only when providing that transport. Both vendor copies are
patched, retaining their inode sizes and SELinux labels.

`fixtures/android-diagnostics.rc` adds periodic service states, `/data` usage
and DRM device names to `hvc1`. `SCARLET_ANDROID_BOOT_OK` is emitted only when
Android sets `sys.boot_completed=1`. Reaching init or the console shell does not
count as success. The boot-completion property also does not validate visible
UI output or input delivery. Existing profiles need updating to gain this
diagnostic service. Inspect `serial.log` for the actual milestone and failure.
For manual runs, set `SCARLET_CROSVM_TIMEOUT=0` (or `run.py --timeout 0`) to
disable the time limit and stop with Ctrl-C. Panic/failure detection and success
markers still end the run; only the elapsed-time cutoff is disabled.
The Android 15 fixture has reached second-stage init, activated APEX packages,
and mounted writable userdata through device-mapper. Use a release Scarlet
kernel; nested TCG boot remains slow. With the GPU option, the guest exposes
`/dev/dri/card0` and `renderD128`, but SurfaceFlinger still aborts with
`EGL_NOT_INITIALIZED`: the SDK's Ranchu graphics stack expects an OpenGL ES host
service that this fixture does not provide. Full Android boot and UI output
remain unverified. Use the separate Cuttlefish software-renderer profile below
to avoid this dependency. Large SDK files stay in local build artifacts.

An interrupted first boot can also produce `/data/data` encryption-policy
mismatch messages on the next run. Android 15 vold has a recovery path for an
empty directory left with the previous unsaved CE key. This message alone is
not evidence that the whole userdata disk needs formatting. Preserve metadata
and userdata together when copying a profile; do not regenerate either over
an existing run's state.

## Cuttlefish guest software renderer (experimental)

`tools/prepare-cuttlefish.py` prepares a separate Android 17 profile using
guest ANGLE, SwiftShader (`vulkan.pastel`), minigbm and the Ranchu composer in
client-composition mode. This uses the Cuttlefish HALs bundled together in
[build 16373615, aosp_cf_arm64_only_phone-userdebug](https://ci.android.com/builds/submitted/16373615/aosp_cf_arm64_only_phone-userdebug/latest).
Download `aosp_cf_arm64_only_phone-img-16373615.zip` from that build. Its published
MD5 is `46f44b3494d54d64c39af2df619f9df3`; preparation pins SHA-256
`051caf8072ba9fb417e05999de2984752e44e13ce70b6c49c669f0a73db85c18`.
This is a different image from the Android 15 SDK fixture above.

Supply an existing **GPU-enabled** crosvm staging tree, for example from
`build.py --gpu --output projects/aarch64-limine-crosvm/.scarlet/base-gpu`.
Preparation requires
Python `lz4`, `mkfs.ext4` and the Scarlet toolchain, but no Docker or privileged
mounts. It refuses an existing output directory and leaves `.scarlet/active`
unchanged (`prepare-os.py --no-activate` is also available independently).

```sh
python3 projects/aarch64-limine-crosvm/tools/prepare-cuttlefish.py \
  --image-zip /path/to/aosp_cf_arm64_only_phone-img-16373615.zip \
  --base-staging projects/aarch64-limine-crosvm/.scarlet/base-gpu/staging \
  --output projects/aarch64-limine-crosvm/.scarlet/cuttlefish-swiftshader
python3 projects/aarch64-limine-crosvm/tools/run.py \
  --kernel projects/aarch64-limine-crosvm/bsp/target/aarch64-unknown-none-elf/release/scarlet \
  --staging projects/aarch64-limine-crosvm/.scarlet/cuttlefish-swiftshader/staging \
  --output projects/aarch64-limine-crosvm/.scarlet/cuttlefish-run \
  --memory 8G --timeout 1800 --success-marker SCARLET_ANDROID_BOOT_OK
```

The guest has 2 GiB RAM, one vCPU and an 800×600 virtio-gpu 2D display. Fresh
userdata is 1536 MiB, metadata is 64 MiB, and misc is 4 MiB. Preserve the writable
GPT disk as one unit. Unused tail capacity is trimmed from the original 8 GiB
super image, updating device/group sizes and SHA-256 checksums in all six LP
metadata copies. Logical partition extents remain unchanged. Scarlet's current
ext2 inode-size handling cannot serve the original 8 GiB file. Each staged disk is below 2 GiB to
avoid the `mke2fs -d` copy overflow in the development environment's e2fsprogs
1.47.3. Boot/init_boot/vendor_boot partitions are included for first-stage init.

The first-stage fstab mounts logical partitions without AVB. SELinux is permissive.
Two uncompressed vendor init-file payloads and the sensors sub-HAL list are
patched at their original lengths, retaining EROFS metadata and SELinux labels.
Sensors report an empty list. Bluetooth, UWB, Thread, NFC, lights, the modem/RIL
and remote OEM-lock APEXes are excluded along with their VINTF declarations because no corresponding host
control services are attached. The ramdisk includes a
Linux bootconfig trailer and explicitly selects local KeyMint/Gatekeeper,
camera and composer APEX implementations. This is a bring-up fixture, not a
verified-boot image or a complete replacement for `launch_cvd`. The compact,
read-only system disk is not suitable for OTA updates.

`hvc0` carries the kernel/console, `hvc1` periodic framework service state and
the real `sys.boot_completed=1` marker, and `hvc2` main/system/crash errors.
Diagnostics also query the SurfaceFlinger GLES renderer. The completion hook
attempts a PNG screen capture, encoded as base64 between `SCARLET_FRAME_BEGIN`
and `SCARLET_FRAME_END`; a boot marker alone does not certify that capture.
Timestamped Android Rust HAL `panicked at` records do not count as a kernel panic in the runner;
Scarlet/Linux kernel panic markers still stop it. The outer
QEMU remains headless; the crosvm stub display does **not** present frames in an
SWS window. Framework boot, guest rendering and visible presentation are
separate milestones. Use `--timeout 0` for a manual unlimited run. For subsequent
runs, reuse the generated `boot.img` and `rootfs.ext2` with `--boot-image` and
`--rootfs-image` instead of rebuilding them and resetting guest disk state.

Host format/parser and panic-classification checks:
`python3 -m unittest discover -s projects/aarch64-limine-crosvm/tests -v`.

Release/TCG verification reached a stable SurfaceFlinger GLES context reporting
**ANGLE / Vulkan 1.3.0 / SwiftShader Device (LLVM 16.0.0), OpenGL ES 3.1**.
An uninterrupted normal TCG run also started SystemServer, but did not reach
`sys.boot_completed=1` within 900 seconds. Android framework boot and a valid
screen capture remain unverified. Earlier clock jumps occurred while the host
was asleep and do not establish a Scarlet timer defect.

## Display integration

The default crosvm build has no GPU feature. `build.py --gpu` enables upstream's
basic virtio-gpu 2D feature and exports its Wayland library dependencies. Pair
that staging tree with `prepare-android.py --gpu-2d --base-staging <tree>` (or
`prepare-os.py --gpu backend=2d,width=800,height=600`). This is an experimental
guest DRM device using crosvm's stub display when no display server is attached;
it supplies neither Android's GLES renderer nor an SWS window. Scarlet supports
the duplicated epoll descriptors and nested readiness needed by this backend.

A release/TCG run with the fix reached `/dev/dri/card0` and `renderD128`, and
the sensor service remained running without the Ranchu transport abort. It
still hit the SurfaceFlinger EGL abort described above; a 2D DRM device does
not implement the SDK's OpenGL ES host service.

A proposed accelerated path is:

```text
Android guest GPU driver -> virtio-gpu -> gfxstream in crosvm
  -> host Vulkan loader -> SGFX Vulkan ICD -> SGFX GPU backend
  -> SWS presentation
```

The ICD alone does not provide crosvm's window/input backend. A first display
milestone can use virtio-gpu 2D scanout copied into an SWS surface, followed by
shared GPU images and gfxstream acceleration. These paths are not implemented
by this fixture.

The [existing SGFX integration](../../docs/graphics/vulkan-games.md) already
provides Vulkan presentation to SWS. Its Linux ICD is currently built against
musl, while this crosvm uses glibc: renderer/loader/ICD dependencies must use a
compatible runtime, or a separate renderer process needs an explicit protocol.
The current ICD exposes a bounded Vulkan 1.0 implementation; gfxstream feature
probing and required entrypoints must be checked before claiming compatibility.
External-memory/semaphore FD support is not currently advertised by that ICD;
copy paths and shared-image integration need separate validation. See
[gfxstream](https://github.com/google/gfxstream) for the renderer transport.
Guest OpenGL ES also needs a compatible path, such as guest ANGLE translating
GLES to Vulkan; selecting host Vulkan does not automatically translate every
guest graphics API.

## Linux ABI regression probes

`fixtures/positional-io.c` is an AArch64 Linux ABI integration probe. Compile it dynamically
with the Docker builder's glibc and install it as `usr/bin/crosvm` in a **private
copy** of the base staging tree; run with
`--success-marker SCARLET_POSITIONAL_IO_OK`. It checks cross-page iovecs, empty
elements, offsets, EOF, access errors and descriptor passing with non-consuming
`MSG_PEEK | MSG_TRUNC`. It uses `/guest/positional-io` on the private ext2 disk.
Do not replace the installed crosvm binary with this probe.

## Compatibility changes exercised

- Linux exec supplies the sixteen bytes referenced by `AT_RANDOM` for glibc.
- memfd size seals support crosvm's fixed-size RAM. WRITE/FUTURE_WRITE seals
  remain unsupported pending writable mapping accounting.
- `signalfd4` provides mask updates, blocking/nonblocking reads, and readiness.
  Reads use the reader's pending signals, including after dup/exec/fork.
  SIGKILL/SIGSTOP cannot be consumed. Scarlet's existing signal model still
  shares thread masks/pending state, coalesces real-time signals, and lacks
  detailed siginfo payloads. Records currently expose the signal number with
  SI_KERNEL and zero payload fields.
- TCGETS/TCSETS use the 36-byte kernel termios layout, avoiding an overwrite of
  glibc's syscall buffer by termios2's extra speed fields.
- Unknown Linux syscalls return ENOSYS, allowing glibc's clone3-to-clone fallback.
- KVM advertises and honors `immediate_exit`, returning EINTR after completing
  any outstanding MMIO read and before guest entry.
- VGIC entry disables continuous group-enabled maintenance requests. Per-LR
  EOI notifications used by IRQFD resampling remain enabled when armed.
- HVC64 preserves the already-advanced ELR; SMC64 advances its trapped ELR.
  Guest general registers are not rewritten by a PSCI helper workaround.
- Guest ICC_SGI1R writes queue SGIs for the selected virtual CPUs; they are
  never sent to the host GIC. Tested OS fixtures use one vCPU.
- KVM shutdown/reset events use Linux's event types 1 and 2.
- Local socket peeking preserves payload and passed handles; seqpacket
  `MSG_PEEK | MSG_TRUNC` reports the full next-record length for crosvm Tube.
- `preadv`/`pwritev` provide bounded gather/scatter I/O without moving the shared
  file position, enabling crosvm's virtio-block disk backend.
- Epoll descriptors can be duplicated with shared registration state and
  independent descriptor flags. Nested epoll readiness supports crosvm's GPU
  display wait context; self-registration and cycles are rejected.

References: [Linux KVM API](https://docs.kernel.org/virt/kvm/api.html),
[signalfd](https://man7.org/linux/man-pages/man2/signalfd.2.html), and
[kernel termios layouts](https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/termbits.h).
