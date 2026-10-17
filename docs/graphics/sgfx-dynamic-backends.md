# Native SGFX drivers

Native AArch64 and RISC-V64 images load VirGL from
`/lib/sgfx/libsgfx_scarlet_virgl.so` through `/bin/scarlet-ld`.
The adjacent `scarlet-virgl.sgfx-driver` manifest selects backend ABI 2.
RISC-V32 retains its compatibility backend and only prepares the coordinated
SGFX source configuration; no driver library or manifest is installed there.
Userspace dependencies explicitly enable `backend-dynamic`. Native 64-bit
VirGL and Maxwell use installed drivers. Adreno remains built into the clients
until its dynamic driver is available. Integration CI rejects static VirGL and
Maxwell implementations anywhere in the native 64-bit userspace dependency graph.
The native 64-bit VirGL static feature has been removed; RISC-V32 continues
to select its compatibility backend through `backend-scarlet-virgl`.
Shared NV12/YCbCr image import is supported by the Maxwell backend. The VirGL
backend does not implement that import path.

Use a Scarlet compiler with native cdylib support, DSO-safe standard-library
TLS, and the GNU ELF OSABI fix (`petitstrawberry/rust` commit `71dd0425890`).
The linker can tag retained constructor sections with OSABI 3; the native
compiler accepts that tag and marks its completed executable/shared-object
output as Scarlet OSABI 83.

For the local compiler, enter the usual Nix development shell and run from
the Scarlet repository:

```sh
source scripts/scarlet-rust-dev.sh
scarlet-rust-use-local ../rust-scarlet
```

The local stage1 sysroot needs both the host libraries (including
`proc_macro`) and libraries for the selected Scarlet target. Cargo and native
build tools continue to come from the development shell. Return to the
packaged compiler with `scarlet-rust-use-cached`.

The `desktop` bundle includes `bundles/sgfx-native` as its first layer, before
`base`, `cli-utils`, and desktop userspace packages. Projects selecting
`desktop`, `full`, or `full-buildroot` receive it through that bundle in their
rootfs. Initramfs compositions and projects without `desktop` include
`bundles/sgfx-native` explicitly before their userspace packages.
The source-owned SGFX recipe declares its driver as a Cargo cdylib layer.
The SDK builds, hashes and stages it; SGFX's build.rs owns its native link flags.
A small configuration/install step writes the driver manifest and
`.scarlet/sgfx/userspace.toml`, selected by the project's `userspace.cargo-config`.
Published UI/backend dependencies retain their shared SGFX core revision.
This config supplies the linker input needed for `scarlet-ld`'s `dlopen`, `dlsym`
and `dlerror` imports without replacing those Git dependencies with local paths. Driver discovery uses `dlopen`; applications have no
`DT_NEEDED` dependency on the VirGL plugin.

Build through the SDK so all package outputs use the project cache:

```sh
nix develop --command cargo scarlet image \
  --project projects/aarch64-limine-full --release
```

The SDK owns source checkouts under the project's `.scarlet/cache/git`, Cargo
outputs under `.scarlet/cache/target/<canonical-source-path-sha256-prefix>`, and
Cargo's registry/Git dependency cache under `.scarlet/cache/cargo-home`.
Image contents are assembled under `.scarlet/staging/<image-name>`, then written
to the image output paths declared in `scarlet.toml`. These are generated data,
not source editing locations. Build recipes and source changes remain in their
repositories. No build output is required in the Scarlet repository's top-level
`target`, `.cargo/target`, or an `artifacts` directory.

Python comes from the Nix shell (3.11 or newer) for the configuration step.
SDK Cargo output stays in the project package cache; the staged driver is the
linker input for applications. The generated userspace configuration stays in
`.scarlet/sgfx/userspace.toml`. No recipe computes cache hashes or performs Git
checkout management. The build uses the selected compiler and does not rewrite
an ELF header or change a packaged sysroot.

Run `sgfx-probe` in the guest to see `linkage: dynamic` and the loaded library
path. Missing or incompatible drivers fail device creation without falling
back to static VirGL. `SGFX_DRIVER_PATH` overrides manifest search directories
and `SGFX_BACKEND` selects a manifest name.

## Native Vulkan

`user/lib/scarlet-vulkan/bundle.toml` owns the Native Vulkan dependency and
installation definitions and is included by the existing `sgfx-native` bundle.
The SDK stages the AArch64 Khronos loader at
`/lib/libvulkan.so.1`, the shared SGFX ICD at `/lib/libvulkan_sgfx.so`, and its
manifest at `/share/vulkan/icd.d/sgfx.json`. Both libraries are Release Native
ELFs with OSABI 83. The staged libraries were audited for ELF TLS, unexpected
imports, unsupported relocations, and leaked CRT/Rust exports. Native loader calls use immediate
binding and global scope, matching the resident `scarlet-ld` contract.

The loader port lives on [Vulkan-Loader's Scarlet branch](https://github.com/petitstrawberry/Vulkan-Loader/tree/scarlet), pinned at
`cabc0232da5ee6bbd1814acdfd6188714a1a1e4d`. Headers are pinned to Vulkan-Headers
1.4.365 (`c46850864f4661461b0f6cb9922c058ffea4915e`). The native driver/ICD package recipe is pinned at SGFX commit
`fdb50cf2056040b974ff8faeedc2c0f7d0e3689f`; it adds native link/package definitions
to core revision `20bb5b8cc19445520d08c31c3971ac06244bc1d9` without changing its IR.
The Native std allocator-owner fix is published in
[the Rust fork](https://github.com/petitstrawberry/rust/commit/26d00a8a3a8489d73f5ae3575880e9f915dfd0f2).
The packaged toolchain records its separate Native std commit and exposes it in
verbose compiler information so Cargo recompiles dependencies after a runtime
update. The compiler commit itself remains unchanged.

`vulkan-canvas-demo --probe` creates and destroys three complete GPU lifecycles.
On 2026-10-13, the native Khronos loader discovered the SGFX ICD, uploaded the
texture, executed GPU draws, read back two distinct frames (72,005 and 72,475
foreground pixels, 1,240 colors each), and destroyed resources, device and
instance successfully in all three cycles. `VK_DRIVER_FILES` selected the
requested manifest; a nonexistent manifest produced a driver-detection error.
These observations establish the exercised offscreen path, not Vulkan
conformance or accelerated WebKit rendering.

Myrica/WebKit still rasterizes through Cairo. The next work is to establish the
formats, blending, stencil and synchronization capabilities needed by Skia
Ganesh Vulkan, connect WebKit image buffers/compositing to Native GPU image
sharing, and verify real pages. Video decoding/playback is not implemented by
this loader milestone. The Native Vulkan loader is currently packaged only on
AArch64; other demo targets retain their existing linked ICD path.
