# Vulkan application compatibility and Scarlet execution

`vulkan-canvas-demo` renders an indexed, textured cube with SPIR-V shaders,
D32 depth testing, a dynamic uniform descriptor with offset 256, and dynamic
viewport/scissor state. Its initial frame is read back through Vulkan into a
BGRA8 staging buffer. The check requires opaque output, clear background
corners, a substantial foreground and at least 64 distinct colors. The actual
GPU pixels are saved as `/tmp/vulkan-textured-cube.tga`.

The Vulkan image is exported once through `VK_SGFX_scarlet_image` and
subsequent frames use ScarletUI's generic `ExternalGpuSurface`. This composition path consumes
the shared GPU image directly. The initial diagnostic readback is not used to
update the UI image.

## Runtime dependencies

```text
Ordinary macOS Vulkan application (vulkan-cube, render_demo, vkQuake2)
  -> installed Khronos libvulkan loader
  -> libvulkan_sgfx.dylib selected by VK_DRIVER_FILES and its JSON manifest
  -> vulkan-sgfx -> canonical SGFX IR -> SGFX WGPU backend -> Metal

Scarlet vulkan-canvas-demo (current linked executable test path)
  -> Vulkan calls through ash and the linked vulkan-sgfx entry
  -> canonical SGFX IR -> SGFX VirGL backend -> /dev/gpu0
  -> VirtIO-GPU -> VirGLRenderer -> host OpenGL driver

Scarlet UI presentation
  Vulkan BGRA8 image -> exported SGFX texture -> ExternalGpuSurface
  -> ScarletUI SGFX renderer -> SWS shared GPU buffer -> SGFX compositor
  -> display surface
```

The Scarlet toolchain currently discards `cdylib` output. This linked test is
not evidence that an existing C game can discover a system-installed Vulkan
ICD on Scarlet. A dynamically packaged Khronos loader/ICD remains necessary
for that deployment model. On macOS the host application uses the existing
Khronos loader; it has no direct ICD-loading branch and does not use
`SGFX_VULKAN_LOADER`.

## Coordinated source revisions

The manifests use ordinary exact Git revisions, and Cargo generates their lock
entries. No source substitution or build-time dependency rewriting is needed.

| Component | Revision |
| --- | --- |
| Canonical SGFX IR | `517529778de9412989f317550dff85dac9eb0598` |
| SGFX facade, VirGL backend and Vulkan frontend | `4511b0ab13821c71c81376f855031b7b1011287b` |
| ScarletUI | `5c738b05333fb9da5d465c5333f0a432582be45b` |
| A618 backend and shader/codegen consumers | `48ae5170fac3a22aa5ad992ae9c8d8833155ca3d` |

## Build and run on Scarlet VirGL

Use the Nix development shell so the Scarlet Rust target and QEMU firmware
are available:

```sh
nix develop
cargo scarlet image --release --project projects/aarch64-limine-full
SCARLET_QEMU_ACCEL=hvf projects/aarch64-limine-full/tools/run_aarch64.sh
```

On Linux use the appropriate accelerator, for example `kvm`, instead of `hvf`.
The image command builds userspace and images; `cargo scarlet build` alone
only builds the BSP/kernel. The desktop bundle installs `/bin/vulkan-canvas-demo`
and starts SWS automatically. In the guest shell:

```sh
export SCARLET_UI_BACKEND=sgfx
vulkan-canvas-demo &
```

The SWS compositor must select its SGFX backend. If starting SWS manually,
use `export SWS_BACKEND=sgfx`, then `sws`. The current Scarlet shell does not
support a shell-style environment assignment preceding an executable.
A successful initial check prints
`PASS: Scarlet Vulkan textured cube GPU readback` with foreground and color
counts, then the application embeds the rotating image between ordinary UI
views. This requires native VirGL support; a software/CPU SWS compositor
cannot consume this shared GPU image.

## Verified release results

On 2026-09-13, the full AArch64 image was built with `--release` and booted
on macOS using HVF and `virtio-gpu-gl-pci`. The guest selected
`SGFX Vulkan (Scarlet VirGL GPU 0)` and reported:

```text
PASS: Scarlet Vulkan textured cube GPU readback: 72005 foreground pixels, 1240 colors; dynamic viewport/scissor, BGRA transfer
[ScarletUI] platform-sws renderer=sgfx backend=scarlet-virgl
ScarletUI presented 1 shared Vulkan cube frames
ScarletUI presented 120 shared Vulkan cube frames
```

The 512x512 TGA was copied to the guest root filesystem and extracted from
the stopped verification disk. Its 1,048,594 bytes decoded to the same
72,005 foreground pixels and 1,240 colors, with opaque alpha throughout.
The actual QEMU window also displayed the textured cube inside ScarletUI;
captures taken at different times showed different rotations. The demo uses
a compact surface so the cube and surrounding text fit the default display.
These checks establish native GPU drawing, Vulkan readback and continuing
shared-image presentation.

Native validation exposed a readback failure for externally mapped render
images. The VirGL backend now reads the physical GPU backing owned by that
mapping, waits for its pending work and applies the requested channel order.
This is the same image exported to the compositor; no substitute CPU image
or separate internal render target is used.

The coordinated changes also passed 332 ScarletUI core tests, 43 SGFX
renderer tests and 23 doctests in release mode. A618's 19 asynchronous
preparation tests and 26 submit-validation tests passed on the host with
the same canonical IR revision. Physical A618 hardware was not tested.
The macOS game check is complete; no Scarlet game run or FPS benchmark is
claimed by the cube integration result.

## API, IR and game limits

This development ICD registers 104 procedures on macOS and 92 on Scarlet,
including 17 command entrypoints. Native capability checks reject unsupported
commands even when their entrypoint is present. These counts describe a bounded Vulkan 1.0
implementation, not conformance or general game compatibility. Canonical IR
adds `SetViewport`, `SetPushConstants`, `WriteTextureMip` and `BlitTexture`
(26 owned command variants), and bind groups allow 32 entries. Command programs
are bounded to 65,536 commands and resource tables to 4,096 immutable bind-group
definitions. Existing sampled-image, sampler and texture-upload IR types are
used by the Vulkan frontend and the VirGL programmable backend.

The current additions cover scalar separate/combined sampled-image
descriptors, coherent staging uploads, dynamic descriptor offsets,
viewport/scissor, common blend operations and one-color/optional-D32 render
passes. On Metal, sampled RGBA8/BGRA8 images support mip chains, complete GPU
blits, sampler mip filtering/LOD clamps, and per-mip uploads/barriers/readback.
Stage-specific push constants support incremental updates up to 128 bytes.
Metal also executes nonindexed triangle strips and signed indexed base
vertices. Indexed strips are rejected before GPU acceptance because implicit
restart would change ordinary Vulkan index semantics. Native VirGL currently
supports triangle lists, single-mip images and no push constants; unsupported
layouts, mip storage and blits return errors. Uploads lower from coherent
CPU-shadow buffer data to owned IR texture writes; they are not a native GPU
buffer-to-image copy implementation.

An unmodified upstream vkQuake2 build on macOS renders the bundled `demo1` map,
weapon and HUD through the installed Khronos loader and this ICD. Twenty-two of
its 23 original SPIR-V modules are accepted. The ordinary game setting
`vk_point_particles=0` selects triangle billboards instead of the unsupported
PointSize path. World frames continue and the game's own screenshot command
produces actual GPU-read images. See SGFX's
[compatibility record](https://github.com/petitstrawberry/sgfx/blob/4511b0ab13821c71c81376f855031b7b1011287b/docs/vulkan-game-compatibility.md)
for the optimized diagnostic build flags, checks and remaining limits.
This result does not establish Scarlet game compatibility: native VirGL mip
storage/blits and push constants, broader resource reclamation, and a packaged
dynamic loader/ICD remain necessary for that deployment.

A618 consumers use the same pinned canonical IR revision, but arbitrary
SPIR-V-to-A618 compilation is not implemented. Host validation and target
compilation do not establish physical Chromebook Vulkan rendering.
