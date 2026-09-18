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

## Tested source revisions

The manifests use ordinary exact Git revisions, and Cargo generates their lock
entries. No source substitution or build-time dependency rewriting is needed.

| Component | Revision |
| --- | --- |
| Canonical SGFX IR for native Rust applications and the linked cube | `517529778de9412989f317550dff85dac9eb0598` |
| Canonical SGFX IR metadata for the Linux/musl ICD | `a18b5a585616f05cf5df0fa5be1977752ceca1ea` |
| SGFX for native Rust applications and the linked cube | `10eb666555e341032eae54cf01433b63ea88f00c` |
| Linux/musl ordinary-loader ICD with shared recordings and native draw bindings | `cdefc6df6068eee2c49f2d0dc5ddc3c3c766a8d5` |
| QEMU Cocoa GL display with window-sized buffers and resize synchronization | `922577606033eade699d231ba7cebcee0d6b92b6` |
| ScarletUI | `e3795f40057ddd223b78b2c2c382aac95eaf906b` |
| Linux SWS C SDK and game platform adapter | `838333bc392f5e345136aa84132c178de2b64c11` |
| Native GPU/SWS SDK | `4b5257897e341a0d0d3136b37d47b0157b9985cd` |
| Full-image kernel and Linux ABI module | `7297aac3e91c09daecd4c09c4c9cb7d57d2e6af3` |
| A618 backend selected by the native Rust revision | `b7bc2c038795527cf538b475649cdeda8e58bdbf` |
| A618 metadata consumers tested on the host with the newer canonical IR | `699787696beaa82ee02614c60fe85eef4714d41e` |

The Linux ICD uses the newer shared metadata API; the native Rust applications
and linked cube retain their listed canonical IR revision. The canonical command
encoding is unchanged, and both builds use the same native GPU/SWS SDK.

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

The [SuperTuxKart 1.5 test recipe](https://github.com/petitstrawberry/scarlet-bundle-linux/tree/main/producer/recipes/supertuxkart)
records the tested [SDL2 SWS port](https://github.com/petitstrawberry/scarlet-sdl2-sws),
ordinary Vulkan loader setup, and game launch without adding game assets to
the public image. STK itself needed no source patch.

The [vkQuake2 fork](https://github.com/petitstrawberry/vkquake2-scarlet/tree/master/scarlet)
adds an SWS platform adapter to upstream commit
`6763f207229f97cffabb6fc2da72017a794b139b`. Its `scarlet/Makefile`
builds the engine, unchanged Vulkan renderer and game
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

For the game's GPU-readback and clean-exit checks, append
`+bind f12 screenshot +bind f11 quit` before the redirection. F12 saves the
game's TGA under `/usr/share/vkquake2/baseq2/scrnshot`; F11 executes its normal
`quit` command. The default Scarlet view prefixes that path with
`/systems/linux-aarch64`.

The game uses `VK_KHR_display`; the tested initial swapchain is 1280x800.
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
Initial world/input checks used the normal full-project release image.
Subsequent release checks with the batched ICD also save the game's own
`quake00.tga` and shut down through its upstream `quit` command with exit status
0 and no remaining SGFX worker tasks. A Linux-shell `if`/`else` also verifies
zero exit for the shared-binding backend without native-shell `$?` expansion. The TGA extracted from the stopped guest
disk is 4,096,018 bytes and decodes to the actual 1280x800 world, weapon and HUD,
with 12,898 distinct RGBA colors and opaque alpha throughout. Sustained FPS and
combat have not been verified.

The native backend now batches consecutive programmable draws sharing an
immutable pipeline, up to 64 draws and a conservative 64 KiB command budget.
Draw order, per-draw constants and first-pass clears are preserved. All 57
Linux release VirGL backend library tests pass, covering batching, shadow writes
and shared binding/index snapshots. No Vulkan procedure or canonical IR command is added by this fix.
One release comparison with four HVF guest CPUs and 8 GiB RAM measured
loading-console submissions of exactly 7,327 owned IR commands: median
`queue.submit` time fell from 388 ms to 200.5 ms, with 36 samples in each run.
Temporary clocks measured CPU submission through acceptance, rather than GPU
completion or FPS. World views differed between runs, and the SGFX device worker
still saturates one CPU. The installed release ICD is rebuilt from committed
source without these clocks. Follow-up clocks on exactly 915 loading-console
draws in 18 chunks (the same 7,327 owned IR commands; 36 samples per run) report
backend work including cleanup at 201.5 ms before shared bindings and 60 ms
afterwards. Destruction of lowered drawing events falls from 139.5 ms to 16 ms;
the whole Vulkan queue worker job falls from 274.5 ms to 143.5 ms. Indexed draw
ranges keep independent bounds checks and share immutable constant/texture
snapshots. Ordered identical buffer writes retain their revisions, and native
buffer uploads reuse bounded packet storage. No public API or IR revision is
changed. World views differed between runs; these medians are CPU elapsed times,
not FPS. Producer completion and SWS presentation usually take 0-2 ms, while CPU
recording/lowering and reclamation still dominate. The worker remains busy on
one CPU and playable performance has not been established.

Changed buffer writes now bound the uploaded bytes and preserve neighboring
transport words. Partial uploads require the exact physical predecessor
revision; stale or partially failed storage is repaired from the complete
initialized CPU shadow. Vulkan execution also consumes deferred descriptors,
barriers and readbacks in order, instead of rescanning each entire list at every
command. The 26 Linux release ICD library tests pass, including a position-check
bound for 10,000 commands and repeated trailing insertions.

Three short upstream timedemo runs used the same 1280x800 display, four AArch64
HVF guest CPUs, 8 GiB RAM, game settings and Cocoa GL binary. The demo contains
64 complete network messages from the bundled `q2demo1.dm2` and a normal EOF
marker; the game reports 57 timed frames in every run. Clean release ICDs report
19.0 s / 3.0 FPS with shared bindings, 17.1 s / 3.3 FPS with changed buffer ranges,
and 15.7 s / 3.6 FPS with ordered Vulkan insertions. These single short runs
confirm modest improvement and remain too slow for normal play. The private
copied demo prefix is not installed in the normal image. To time the full
bundled demo, use:

```sh
abi-run linux-aarch64 /bin/sh /usr/games/vkquake2 +set timedemo 1 +demomap q2demo1.dm2 +bind f11 quit </dev/null &
```

The later ordinary release checkpoint at SGFX
`cdefc6df6068eee2c49f2d0dc5ddc3c3c766a8d5` retains shared command recordings,
canonical binding suppression, native binding caches and per-pass constant and
texture scratch storage. Replaying the session's recorded source changes rebuilt
its ICD byte for byte against the saved 4.2 FPS binary. A fresh run with the same
57 timed frames, HVF, four guest CPUs, 8 GiB RAM and 1280x800 swapchain reported
13.0 s / 4.4 FPS. These are single short runs and remain too slow for normal play.
The ordinary ICD is installed in both the project's normal rootfs copy layer
and release image; the private demo prefix remains confined to verification disks.
The game returned status 0 through its upstream quit command with no remaining
Vulkan or native GPU workers. The restored backend passed 61 release library
tests and a Scarlet-target release check.

One final clean-image boot also reports an invalid-return user fault in the
native GUI `/bin/scarlet-shell` before the game is launched. The serial shell
remains usable for the game checks. This separate desktop startup failure is
recorded in the verification log; these game results do not establish desktop
stability.

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

## Display resize

SWS samples a retained shared GPU image into the current window geometry.
When those extents differ, its image-space damage now repaints the full sampled
window, preventing old frames in the expanded part of a fullscreen display.
Unscaled images retain their bounded partial damage. Six damage-bookkeeping
tests cover expansion, shrinkage, partial updates, movement and visibility.

The pinned QEMU Cocoa frontend converts guest-coordinate bounds through window
points and explicitly sizes its OpenGL buffer in actual backing pixels. It also
completes replacement snapshot storage initialization before another shared GL
context uses it. Native checks expand 1280x800 to 2074x1296, shrink to 986x616 and
repeat both transitions; the game world, weapon, HUD and menu remain fully
visible. The swapchain image remains 1280x800 and SWS scales it to the output.
This verifies retained-image presentation, not swapchain recreation by the game.
The ordinary full image built with the listed QEMU and Linux ICD revisions also
renders its world and menu after a 1280x800 to 1408x880 transition, then exits
through the upstream quit command with status 0 and no remaining SGFX workers.
The installed ICD matches a clean build from its committed source byte for byte.
A second ordinary-image boot saves the game's own 1280x800 world/weapon/HUD TGA
through Vulkan readback: 4,096,018 bytes, 10,849 RGBA colors and opaque alpha.
It also quits with status 0 and no remaining SGFX workers. Automated key checks
use QMP `send-key` with a 1000 ms hold time.

Initial Vulkan images remain limited to 2048 pixels per dimension. Starting
the game on a larger initial display can fail swapchain creation; resizing an
already initialized 1280x800 image does not require a larger Vulkan image.
For this test, start QEMU with `zoom-to-fit=off`, launch the game, then enable
View > Zoom To Fit for window resizing. General larger-image support and
long-running resize stress remain unverified.

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
[compatibility record](https://github.com/petitstrawberry/sgfx/blob/08143f8dcf23cc29065a78a965d629a401106e9b/docs/vulkan-game-compatibility.md)
for the optimized diagnostic build flags, checks and remaining limits.
Native VirGL now has the mip storage/blit, push-constant and dynamic fullscreen
vertex-read support used by the game, and an ordinary Linux loader/ICD package.
The separately recorded Scarlet game checks verify world rendering, console
input, game GPU readback and normal shutdown. Broader input, combat and sustained
performance checks remain necessary.

A618 consumers use the same pinned canonical IR revision, but arbitrary
SPIR-V-to-A618 compilation is not implemented. Host validation and target
compilation do not establish physical Chromebook Vulkan rendering.
