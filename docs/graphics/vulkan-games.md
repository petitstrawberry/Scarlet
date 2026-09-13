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

Scarlet Linux ABI vkQuake2 (ordinary loader/ICD path)
  -> upstream ref_vk.so -> unmodified Khronos libvulkan.so.1
  -> /usr/share/vulkan/icd.d/sgfx.json -> /usr/lib/libvulkan_sgfx.so
  -> canonical SGFX IR -> Naga/TGSI -> native SGFX VirGL backend
  -> explicit Scarlet native object syscalls -> kernel virtio-gpu
  -> QEMU VirGLRenderer -> host OpenGL driver
  -> the same libsws_client_c.so for GPU-image presentation and game input

Scarlet vulkan-canvas-demo (linked executable integration test)
  -> Vulkan calls through ash and the linked vulkan-sgfx entry
  -> canonical SGFX IR -> SGFX VirGL backend -> /dev/gpu0
  -> VirtIO-GPU -> VirGLRenderer -> host OpenGL driver

Scarlet UI presentation
  Vulkan BGRA8 image -> exported SGFX texture -> ExternalGpuSurface
  -> ScarletUI SGFX renderer -> SWS shared GPU buffer -> SGFX compositor
  -> display surface
```

The linked cube integration and ordinary Linux C application are separate
checks. The native Scarlet Rust target currently discards `cdylib` output;
the deployed Linux/musl ICD is built as an ordinary Linux shared library and
uses the explicit native syscall namespace for GPU objects. Musl and Rust std
continue using Linux ABI calls. The installed Khronos loader discovers the
system ICD manifest. No private loader or `LD_PRELOAD` is used. On macOS the
application also uses the existing Khronos loader, without an ICD-loading
branch or `SGFX_VULKAN_LOADER`.

## Coordinated source revisions

The manifests use ordinary exact Git revisions, and Cargo generates their lock
entries. No source substitution or build-time dependency rewriting is needed.

| Component | Revision |
| --- | --- |
| Canonical SGFX IR | `517529778de9412989f317550dff85dac9eb0598` |
| SGFX facade, VirGL backend and Vulkan frontend | `10eb666555e341032eae54cf01433b63ea88f00c` |
| ScarletUI | `e3795f40057ddd223b78b2c2c382aac95eaf906b` |
| Linux SWS C SDK and game platform adapter | `838333bc392f5e345136aa84132c178de2b64c11` |
| Native GPU/SWS SDK | `4b5257897e341a0d0d3136b37d47b0157b9985cd` |
| Full-image kernel and Linux ABI module | `7297aac3e91c09daecd4c09c4c9cb7d57d2e6af3` |
| A618 backend and shader/codegen consumers | `b7bc2c038795527cf538b475649cdeda8e58bdbf` |

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

## Linux ABI game installation

Build the upstream `kondrak/vkQuake2` source at
`6763f207229f97cffabb6fc2da72017a794b139b` with the
[SWS platform adapter](../../user/lib/sws-client-c/examples/vkquake2/README.md).
Its ordinary Make target builds the engine, unchanged Vulkan renderer and game
module with `-O3 -DNDEBUG`; it uses C++17 for the upstream VMA allocator and the
null sound driver. Install them into
`projects/aarch64-limine-full/rootfs/systems/linux-aarch64` with `sws-install`.
Supply your own game data there. The adapter README lists the Linux shared
libraries and standard ICD manifest to install in this same tree. The normal
full-project rootfs copy layer includes it; local binaries and assets are
ignored by Git. Build the full image with the release command above.

Linux sees the libraries at `/usr/lib`; the default Scarlet Environment sees
`/systems/linux-aarch64/usr/lib`. Existing `LD_LIBRARY_PATH=/usr/lib:/lib`
resolves the shared dependencies. In the native Scarlet shell:

```sh
abi-run linux-aarch64 /bin/sh /usr/games/vkquake2 +map demo1 </dev/null &
```

The game uses `VK_KHR_display` and the primary display's full 1280x800 extent.
SWS delivers keyboard, mouse, focus and close events to the adapter through the
same shared C SDK connection used by the ICD. The launcher disables point
particles, CD audio and music using upstream settings. Audio output and
windowed display scaling are not implemented by this initial platform port.

Actual Scarlet checks verify automatic ordinary-loader ICD discovery and
60 `VK_KHR_display` presentations with clean shutdown. The upstream game loads
its renderer and game module, initializes the `demo1` server, and renders the
textured 3D world, first-person weapon and HUD through the native VirGL GPU
path. Captures of the actual QEMU window show different player views after
input; an explicit keyboard event opens the game console and pauses the world.
These checks used the normal full-project release image and the coordinated
revisions above. Sustained FPS, combat and a normal game shutdown have not been
verified. The game screenshot command has not been verified on Scarlet;
the world/input evidence is captured from the actual QEMU display.

Initial world loading exposed missing Linux `mremap`; in-place shrinking now
passes four real 16 MiB C checks on Scarlet, including data retention, page
rounding and reuse of zeroed discarded pages. AArch64 range unmapping also now
invalidates the TLB once after removing the range, rather than globally for
every 4 KiB leaf. Physical backing ranges are reclaimed in one registry pass
per unmap request instead of repeatedly scanning all allocations and task pages.
The same four 8 MiB partial-unmap regression passes on Scarlet and reports
4 ms total unmap time, compared with 509 ms before batch physical reclamation.
Retained neighbors and zeroed replacements are checked in both runs. This
microbenchmark does not measure game FPS.

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
The macOS game check is complete. The cube integration result alone does not
establish an ordinary-loader game run or FPS benchmark; the Linux checks above
separately establish game world rendering and console input through the ordinary
loader. Neither run establishes a sustained FPS benchmark.

## API, IR and game limits

The initial development ICD registered 104 procedures on macOS and 92 in the
linked Scarlet path. Linux display WSI additionally registers seven display
procedures and the common surface/swapchain procedures,
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
restart would change ordinary Vulkan index semantics. Native VirGL supports triangle lists and indexed/nonindexed strips without
primitive restart, color mip storage and complete mip blits when device caps
allow them, stage-specific push constants up to 128 bytes, and bounded dynamic
reads of arrays/vectors/matrix columns. Bounded reads lower to integer comparisons
and selects; dynamic stores, runtime-sized indexing and arbitrary loops remain
unsupported. Native mip readback remains limited to supported paths. Unsupported
layouts, resource types and capabilities return explicit errors. Uploads lower from coherent
CPU-shadow buffer data to owned IR texture writes; they are not a native GPU
buffer-to-image copy implementation.

An unmodified upstream vkQuake2 build on macOS renders the bundled `demo1` map,
weapon and HUD through the installed Khronos loader and this ICD. Twenty-two of
its 23 original SPIR-V modules are accepted. The ordinary game setting
`vk_point_particles=0` selects triangle billboards instead of the unsupported
PointSize path. World frames continue and the game's own screenshot command
produces actual GPU-read images. See SGFX's
[compatibility record](https://github.com/petitstrawberry/sgfx/blob/ff6af0531bda21ef27e96c4e22495a4957f86d8c/docs/vulkan-game-compatibility.md)
for the optimized diagnostic build flags, checks and remaining limits.
Native VirGL now has the mip storage/blit, push-constant and dynamic fullscreen
vertex-read support used by the game, and an ordinary Linux loader/ICD package.
The separately recorded Scarlet game checks verify world rendering and console
input. Broader input, combat, shutdown and performance checks remain necessary.

A618 consumers use the same pinned canonical IR revision, but arbitrary
SPIR-V-to-A618 compilation is not implemented. Host validation and target
compilation do not establish physical Chromebook Vulkan rendering.
