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
records the tested [SDL2 SWS port](https://github.com/petitstrawberry/sdl2-sws),
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


## Native WebKit Ganesh contract

The normal AArch64 desktop bundle installs `/bin/myrica-vulkan-probe` through
Myrica's ordinary Cargo producer. The app owns its WebKit source revision;
Scarlet's flake supplies the common SDK runtime only. Build the normal disk
with `cargo scarlet image --project projects/aarch64-limine-full --image disk
--release --locked`, boot with HVF and `virtio-gpu-gl-pci`, then run
`myrica-vulkan-probe` in the standard guest shell.

On 2026-10-14 the release check passed initialized C++ ELF TLS, Rust TLS and
errno isolation across four native threads. Skia Ganesh then drew white, red
and blue regions into an exported BGRA Vulkan image through the installed
native loader, SGFX ICD and VirGL backend. GPU readback matched the expected
pixels in all three independent create/draw/readback/destroy cycles.

The tested WebKit implementation is
[`d05ff7a`](https://github.com/petitstrawberry/WebKit/blob/d05ff7a348a8110934792429b2a87e0aa93de18e/Source/WebCore/platform/graphics/scarlet/vulkan/ScarletSkiaVulkan.cpp),
consumed by Myrica `29babea916b984d99a5fa672e44910271149219d`, with native ICD
`7a62d766818b205fc21a69aae0d72792d4b374aa`. Its reusable context owns one
Vulkan device and Ganesh context; owned surfaces keep that context alive.
The probe holds 128x96 and 160x112 shared images simultaneously, submits both,
duplicates and closes an exported capability, and destroys one image before
submitting the other again. All three cycles passed with this implementation.
Image publication waits for GPU completion. A future embedding must also
wait for its consumer before reusing an image.

The ICD fix accepts explicit same-family image/buffer barriers (`0 -> 0`).
The previous check incorrectly required two `QUEUE_FAMILY_IGNORED` indices,
rejecting Skia's first valid image transition and subsequently losing the
asynchronous queue. All 80 release ICD unit tests and both real Metal GPU
barrier/readback contracts passed. The existing native cube also passed three
GPU readback cycles in the same Scarlet guest, and `myrica --help` exited
normally. The probe's remaining ELF imports are the four ordinary `dl*`
functions and weak `environ`.

The AArch64 desktop recipe now selects `backend-webkit-vulkan` for the normal
Myrica binary and the diagnostic probe. WebCore paints DOM/CSS/text into two
shared BGRA Vulkan images, and ScarletUI imports them directly on its UI thread.
Normal presentation has no CPU page readback or upload. Leased images are not
reused until the UI completes sampling the replacement; two outstanding leases
apply backpressure while input and engine polling continue.

The SGFX implementation adds unsigned 16-bit glyph coordinates, fragment
antialias derivatives, statically used push-constant member ranges, matrix
inverse/determinant and color operations, and partial render-area preservation.
Push-constant allocation retains the declared span, while validation excludes
unused members and SPIR-V wrapper padding. Vector, matrix and array members
still require their full declared range. These checks passed 89 native VirGL
unit tests and 43 codegen tests in release mode (one existing ignored test).
The partial-area contracts additionally passed 84 Vulkan unit tests and an
actual Metal GPU readback of all 8x8 pixels.

The host VirGL library is built from the
[Scarlet fork](https://github.com/petitstrawberry/virglrenderer/tree/dfb43e3191adbc76547a1b33a9acdbf24787d085),
which also contains the macOS core-GL and kqueue fixes previously carried as
local patches. TGSI `FMA` lowers to GLSL `fma` only with GPUShader5 support.
The translator and actual Apple GPU readback distinguished the fused result
`-2^-46` from the unfused result `0`; `MAD` is not used as a substitute.

This is a bounded native rendering profile, not Vulkan conformance or a video
playback implementation. Media and WebGL/WebGPU remain unavailable.

The normal release guest also passed the WebCore page contract at Retina scale
2: actual DOM/CSS text and rounded corners, JavaScript execution and clicks,
two-image backpressure, and unchanged readback of leased images after DOM
mutation. This run used Myrica `95ba981c333ddbf4f1d9b507b878c2f139187936`,
WebKit `93b7d1b288bdd230b33d59beb7b3e3340334443e` and SGFX
`6620598472210a3981cef4053ed5e85c16c3b9ad` with HVF and Cocoa GL.
It establishes the native page rendering contract; live-site compatibility
needs separate checks.

Live-site testing additionally exposed scalar `R32_SFLOAT` vertex input,
per-instance vertex buffers, in-render-pass attachment clears and Skia's
transparent-black decal samplers. SGFX
[`d941683`](https://github.com/petitstrawberry/sgfx/commit/d9416831b946d9a4f83cdcac584d717d8c1d180d)
implements their actual GPU transport. Instance fetch uses divisor one and
advances the native buffer by `firstInstance * stride`; shader instance indices
retain the existing first-instance correction. Vertex and instance bounds are
checked independently, and existing per-vertex ABI words remain unchanged.
Attachment clears preserve raster order and surrounding pixels through GPU
partial passes. Sampler border color remains explicitly bounded to transparent
black. Metal readback checked scalar/instance coverage at every pixel,
overlapping clears and LOAD/STORE preservation, and both nearest/linear border
sampling; the native VirGL packet and descriptor checks passed separately.

The YouTube crash was independently traced in the release ELF to
`GrVkSampler::Create`: a failed Vulkan sampler creation dereferenced a null
YCbCr conversion during cleanup.
[WebKit `bb36d8b`](https://github.com/petitstrawberry/WebKit/commit/bb36d8bc29ce36279da6eba9e02bdf6339d339e0)
uses null-safe release without suppressing the Vulkan error. Native compilation
and null/single-owner/cache-shared cleanup checks passed.

The live-page probe, linked to the release GPU profile, separately verified
loaded titles and nonblank actual GPU readback for `example.com`, `google.com`,
`ichigo.dev`, `abehiroshi.la.coocan.jp` and `youtube.com`. Ichigo's
`lastfm-placeholder` changed from its loading text to real track names through
page JavaScript. Both live-page batches joined the native owner cleanly. These
checks cover loading and pixels, not visual layout approval or video playback.

Yahoo exposed `vkCmdCopyImage` calls during painting.
[SGFX `7164492`](https://github.com/petitstrawberry/sgfx/commit/71644925bc45932d0fe1e7905cf421aa572f4616)
lowers supported same-format, mip-zero, single-layer regions to the existing
bit-exact GPU texture copy. Metal readback matched every byte of 384 source and
destination pixels across BGRA, RGBA and sRGB, including premultiplied alpha and
hidden RGB under zero alpha. Nonzero mips, self-images and aliased storage remain
explicitly unsupported.

The static HTTP fixture also identified one redundant post-layout paint: actual
GPU bytes did not change and no further paints followed.
[WebKit `91dab62`](https://github.com/petitstrawberry/WebKit/commit/91dab62b995a3d18a90a5c61b5b7add21d6957ab)
removes full repaint from unchanged viewport observation. Resize, scroll
and renderer invalidation retain their normal dirty paths. The smoke test
requires both the generation and the actual paint counter to stay unchanged
while idle; it does not add a settling delay or browser readback. The subsequent
native fixture still observed a single unchanged repaint (`2 -> 3`), so this
change alone has not resolved idle invalidation.

The release disk with Myrica `c041dbde85b7565ca1680e69615dab005b10cf05`,
WebKit `91dab62b995a3d18a90a5c61b5b7add21d6957ab` and SGFX
`71644925bc45932d0fe1e7905cf421aa572f4616` passed the four-thread TLS,
three-cycle Ganesh and Retina WebCore page contracts. Yahoo! JAPAN additionally
loaded its real title and produced 557,117 nonwhite GPU pixels, then joined its
native owner without the previous image-copy errors. The HTTP transport now
uses 2–4 bounded worker threads, with compressed entity decoding on a separate
bounded blocking pool; WebCore and JavaScript retain their native owner.
