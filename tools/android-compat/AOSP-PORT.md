# AOSP hosted port: scope and dependency audit

2026-11-03. Objective: ordinary applications using ART JIT and AOT, preserving
AOSP behavior wherever possible. Necessary platform integration belongs in a
small, reproducible source patch set or a device/HAL implementation. Rendering
a window alone does not demonstrate that objective.

## Findings from the actual inputs

The pinned SDK image is Android 13, build `TE1A.220922.034/10940250`, ARM64.
`prepare-aosp.py` now preserves all 17 userspace archive members unchanged,
including original partition images and packaged APEXes. It excludes only the
Linux kernel. The preparation manifest explicitly does not assert a boot.

`aosp_inventory.py` checks the prepared inputs and the four extracted partition
extents against those original bytes. It collected 116 init configuration files,
27 APEX packages and 133 service declarations, including APEX `etc/*.rc` files.
Those declarations include alternate Zygote configurations, disabled services
and property conditions; they are **not** 133 services required simultaneously.

The saved successful `aegis-jit-padded` fixture contains 33 manifest-listed init
files, all matching their recorded hashes. Its 165 service declarations include
120 generated application-start attempts. Current source has already replaced
those attempts with one launch after user unlock; do not recreate the old launch
scheme. The saved SystemUI and Launcher3 APK hashes match the original SDK
partition APK hashes. The rendered application observation remains valid, but
that run does not prove ordinary warm boot or user unlock completed.

Evidence: [inventory.json](https://github.com/petitstrawberry/Scarlet/blob/86ba4d1ebc1eed07af57455e34899fa36c3c4237/tools/android-compat/checkpoints/2026-11-03-aosp-port-audit/inventory.json).
The inventory covers declarations and saved manifest paths, not Binder call
reachability, every possible imported path, or complete syscall compatibility.

## Why Waydroid is relevant, and what not to copy

Reference tree: `waydroid/android_vendor_waydroid` at
`1b95b85221f4faaa357932fa5e93eacb7430f636`, Android 13 patch directory
`waydroid-patches/base-patches-33`.

Waydroid mounts images on the host, supplies devices, then launches `/init` in
LXC. Its init patches bypass first-stage partition mounts and enter second stage
without SELinux setup. Another patch changes `mount_all` to report host-mounted,
unencrypted data and drive the usual init transition. This is an explicit hosted
port, not an unmodified emulator boot.

References:

- [Host image mounts](https://github.com/waydroid/waydroid/blob/main/tools/helpers/images.py)
- [LXC launch and devices](https://github.com/waydroid/waydroid/blob/main/tools/helpers/lxc.py)
- [Pinned init integration patches](https://github.com/waydroid/android_vendor_waydroid/tree/1b95b85221f4faaa357932fa5e93eacb7430f636/waydroid-patches/base-patches-33/system/core)

Its patch set also disables SELinux checks, changes framework behavior and
tolerates missing BPF functionality. Those are not automatically Scarlet's
requirements. Scarlet already has Shield, native RPC, RT scheduling, process
scopes and namespace primitives. Review each boundary against those facilities.

Full export to ext2 and selecting a subset of Android files are independent
choices. Preserving all files, links, owners, modes and attributes is required
regardless of the outer storage format. A native ext4 driver is not a prerequisite
merely because Android was distributed as ext4 images. Conversely, retaining the
SDK's original first-stage fstab would require its actual ext4/logical-volume
mount path. That is a different scope from a hosted Android product.

## Existing integration that must be replaced coherently

| Area | Evidence in current code / saved fixture | Required hosted-port treatment |
| --- | --- | --- |
| Init and services | `prepare-boot-profile.py` generates an explicit rc import list; `host_services.py` selects 15 host providers. `init-scope-probe.c` starts second stage with two preloads. | Preserve the platform's normal init directory imports. Supply a defined device configuration and host handoff; eliminate the diagnostic supervisor/preloads from normal boot. |
| Data and unlock | Generated rc manually connects `zygote-start` to `nonencrypted`; `/data` is prepared externally. The saved warm tests remain `RUNNING_LOCKED`. | Define host-owned data readiness at the init/vold boundary. Let stock user-0 setup, LockSettings, Gatekeeper/KeyMint and ActivityManager perform unlock; do not call a custom Java unlock helper. |
| APEX/classpath | The fixture flattens packages, sets `ro.apex.updatable=false` and reconstructs `BOOTCLASSPATH`. | Use a consistent AOSP product build with its generated classpath and APEX mode. Flattened APEX was previously selected; validate its product configuration rather than only overriding a property in a different image. |
| Media | No `media`/mediaserver, mediaextractor or mediametrics service declaration in the saved fixture; APEX media.swcodec is present. | Restore the platform media service chain as a group. MediaCodecList falling back from missing `media.player` into direct HAL discovery is not fixed by granting MediaProvider extra HAL permissions. |
| Power | `android-runtime-shim.S` intercepts PowerHalController methods. The source image declares `vendor.power-default`, absent from the fixture. | Provide a Power HAL that reports actual hosted capabilities and delegates host power policy. Remove the client-method interception. Inspect the example HAL before treating it as a suitable implementation. |
| Audio | `host_services.py` alters restart relationships, removes capabilities/profiles, adds `audio-start-helper`, modifies the audio policy and preloads audio modules. | Implement/configure the Scarlet audio HAL boundary; keep AudioFlinger/service lifecycle intact. Recheck formerly missing capabilities against the current kernel. |
| Network | Managed netd rc suppresses the BPF wait; a replacement iptables program is staged. `generic/bpf.rs` synthesizes objects from path names and returns success for updates/attachment without packet execution. | This is incomplete network semantics. Choose a real host network adapter or native enforcement path, and remove synthetic success along with its clients. Do not describe these handles as working BPF. |
| Logging | The supervisor binds logdw; `init-log-probe.c` interposes logging and standard I/O. Stock logd is absent from the saved service declarations. | Use stock logd and ordinary external log collection. Keep invasive tracing opt-in for diagnosis only. |
| Graphics | Stock SurfaceFlinger and CPU-buffer HAL integration have produced SWS output. The fixture selects adapters and custom device declarations. | Keep this working implementation as the Scarlet graphics provider, expressed through product/HAL configuration. Test native-window input, frame delivery and app lifecycle together. |
| Metadata/policy | Full inode metadata is not preserved by the old selected-file exporters. Native ext2 xattrs are currently an in-memory map; Linux exact modes are also held separately. Shield rules are generated from explicit fixture selections. | Full import must preserve metadata and map file labels into Shield consistently. Derive policy for the complete hosted product rather than adding grants after each failing service. |

Relevant local code: `tools/android-compat/{prepare-boot-profile.py,host_services.py,
init-scope-probe.c,init-log-probe.c,android-runtime-shim.S}`, and
`kernel/src/{abi/linux/generic/{fs.rs,bpf.rs,mode.rs,credentials.rs},
fs/vfs_v2/drivers/ext2/{mod.rs,node.rs}}`.

## Dependency order and acceptance evidence

1. **Reproducible product and host handoff.** Pin an AOSP manifest/revision,
   retain normal framework/apps/runtime, and express hosted init/data/device
   differences as reviewable patches and product configuration. Inventory every
   changed payload. Do not mix a guessed AOSP tag with the SDK's binaries and
   call it a matching source rebuild. The previously inspected `android-13.0.0_r1`
   sources are reference material, not proven source identity for build 10940250.
2. **Whole storage and metadata contract.** Complete files and generated
   classpaths, host-owned data, the selected APEX activation mode and Shield
   labeling must agree before boot. A bind-mounted directory is not proof that
   encryption, quotas, verity or FUSE work. Current Linux mount translation
   supports tmpfs, bind/propagation, cpuset/cgroup2 subsets; it is not a general
   Linux block-filesystem mount implementation.
3. **Service groups and actual capabilities.** Validate media/codecs, power,
   audio, logging, credentials/resource profiles, networking and storage as
   groups. Stock service declarations request capabilities beyond those in the
   current ABI mapping. Resolve the hosted ownership and supported subset
   explicitly, before full boot; do not remove each declaration on failure.
4. **Normal lifecycle and applications.** Require normal boot completion,
   `RUNNING_UNLOCKED`, working Launcher/SystemUI, package install and one launch,
   input and resume, actual JIT compilation, and dex2oat-produced AOT artifacts
   used on a subsequent launch. Preserve Aegis as a regression case; do not
   replace general-app validation with a custom sample or a screenshot.
5. **Comparable timing.** Same kernel, product, data seed, 4 vCPUs, 4 GiB and HVF;
   distinguish cold/warm boot and compilation. Normal log collection, no repeated
   launch loop or forced compile hidden in boot. Stop the VM after validation.

Static inventory cannot prove these runtime requirements. It avoids spending a
full boot to discover a missing service definition or a deliberately bypassed
subsystem. Focused tests precede each full integration run.

## Build environment and current status

No new Android boot or ext4 implementation was made during this audit. Existing
successful data and kernel fixes remain intact. The host had about 3.6 GiB free;
the configured OrbStack Docker endpoint was unavailable. A full AOSP source tree
and functioning Linux build environment were not established in this worktree.
The build location is requested from the user; source/patch review and tooling
can proceed without starting another VM or downloading an incomplete tree.

Reproduce the input inventory (read-only; temporary APEX payloads are removed):

```sh
python3 tools/android-compat/prepare-aosp.py \
  --sdk-image .build/android-services/arm64-v8a-33_r02.zip \
  --output .build/android-services/aosp-stock-images-v1
python3 tools/android-compat/aosp_inventory.py \
  --images .build/android-services/aosp-stock-images-v1 \
  --partitions .build/android-services/sdk-partitions \
  --fixture-profile .build/android-services/aegis-jit-padded/boot-profile.json \
  --fixture-rootfs .build/android-services/aegis-jit-padded/rootfs.ext2 \
  --output .build/android-services/aosp-init-audit.json
```

For existing prepared inputs, use `prepare-aosp.py --verify --output ...`.
Both tools refuse to overwrite their output. `debugfs` must be on PATH (or use
`aosp_inventory.py --debugfs PATH`). The partition offsets are specific to the
hash-pinned SDK archive; this is not a general Android image unpacker.
