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

This earlier profile was a bounded native rendering profile, not Vulkan
conformance or video playback. Media and WebGL/WebGPU were unavailable in that
revision; native media bring-up is recorded below.

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


## Native media bring-up (2026-10-15)

The previously verified release disk with Myrica `e2a8b203`, WebKit `cf35e35`, kernel
`f7afdc393` and SGFX `7164492` uses `scarlet-video-client` for H.264 through
`/dev/video0`. MP4/fMP4 demuxing and AAC remain in the shared codec library;
the normal browser has no CPU H.264 decoder or software fallback. Decoded NV12
planes are uploaded through Skia's YUV textures and converted on the GPU. The
current mapped-plane handoff is not a zero-copy video-image import. Page
presentation continues through shared BGRA8 GPU images without browser CPU
framebuffer readback.

On macOS, build the existing host producer with
`cargo make build-vhost-video-videotoolbox`, then enable
`SCARLET_VHOST_USER_VIDEO=1` and `SCARLET_QEMU_AUDIO=1` in the ordinary runner.
The former adds the VideoToolbox-backed device; the latter supplies SAS audio.
A missing or unsupported video device returns a media error.

This release guest passed the four-thread TLS, three-cycle Ganesh and Retina
WebCore contracts. Actual GPU readback checked 10,752 I420/NV12 pixels across
BT.601/BT.709 full/limited range, with padded Y and chroma strides, RGB tolerance
of two and opaque alpha. These checks exercise Skia's actual YUV texture path,
not a CPU RGB conversion.

The HTTP fixture additionally passed CSS/image loading, exact idle revision and
paint-count stability (`2`, `2`), pointer movement, Retina resize, history, text
editing, POST/reload, native scrolling, one damage paint, cookie/storage/blob
iframe loading, frameset child pixels, and native owner shutdown. The queued
image-load notification fix resolves the previously observed idle repaint
without adding a settling delay.

Native decoding produced actual NV12 frames for the self-generated video and
YouTube's 320×240 stream. Continuous playback has not yet passed. Real
`youtube.com` homepage loading reproduced a null dereference in
`MediaPlayer::networkStateChanged`, called from the Scarlet backend destructor
during media sniff/reload. This is tracked separately from GPU rendering and
video decode.

The subsequent normal release image uses Myrica `d634b193`, ScarletUI main
`1c9c7b33`, WebKit `4969dc35`, SGFX `2bd22fd8`, and kernel `d6af352cf`.
The actual initramfs and GPT root partition were independently inspected:
SGFX plugin, Vulkan ICD, and Myrica bytes match staging and the SDK lock.
The guest again passed all three Ganesh/YUV cycles and the Retina DOM/CSS/JS,
input, immutable-image and backpressure contracts. YouTube's actual GUI
still exits during loading; continuous playback remains an open validation
item. No screenshot or visual-layout approval is claimed.

ScarletUI main now consumes raw canvas mouse events without implicitly
repainting; state changes still request painting. All native desktop UI
consumers use this same revision. The kernel presentation path releases IRQ
guards before waiting, and transfers image/device/request ownership into a
driver registry before scheduling. Task death releases admission, completed
leases retire outside IRQ guards, and failed DMA remains quarantined. The
source audit, 28 production queue/control/registry regressions and native
release plus kernel-test compilation passed. Guest mouse-performance and
forced-exit stress validation are still pending. Myrica also restores the
previous WebKit wheel direction.


The next HVF release run reproduced an abort on the supplied YouTube watch
URL. Native exit-group diagnostics recorded task 138, status 134,
PC `0x7134a60`, LR `0x3552a90`, and saved return addresses `0x55465ac`,
`0x38e7558`, `0x38e732c`, `0x3d1716c`. Symbolization against the actual staged
Myrica executable identified `WTFCrashWithInfo`, `GraphicsLayer::create`,
`RenderLayerCompositor::ensureRootLayer`, `enableCompositingMode`, and
`Document::setActiveViewTransition`. Myrica's main thread then encountered the Rust abort trap while shutting down
after its engine owner exited. This capture identifies
a View Transition compositing request, rather than a decoded NV12 frame fault.

[WebKit `74b12ed`](https://github.com/petitstrawberry/WebKit/commit/74b12edd09065b36efaf753ef361d4302ef64f18)
uses the existing View Transition skip algorithm when accelerated compositing
is disabled: same-page update callbacks still run and their promises settle;
cross-document transitions resolve to `SkipTransition`. The port currently
rasterizes the page through Ganesh/Vulkan but has no composited graphics-layer
factory. The change does not replace that missing compositor with dummy layers
or change native video decoding. The Myrica regression fixture checks callback
execution, promise outcomes, the cleared active transition, and actual GPU
pixels after a DOM update. The HVF release guest passed this regression:
callback asynchronous and exactly once, `ready` rejected with
`InvalidStateError`, `updateCallbackDone` and `finished` fulfilled, active
transition null, and actual GPU pixels/revision changed. All existing static
HTTP fixture stages also passed, including unchanged idle counters (`2`, `2`)
and pointer movement without a new paint. Live YouTube playback verification
is in progress.


After that fix, the watch-page probe reached the native decoder but reproduced
a second status-134 abort. Its exact executable resolved the saved callers to
`CanMakeCheckedPtrBase::crashDueToCheckedPtrToDeadObject`,
`RunLoop::TimerBase::~TimerBase`, `MediaPlayerPrivateScarlet` destruction,
`ThreadSafeWeakPtrControlBlock::strongDeref`, and the settled media sniff/reload
promise. Inspection found that `RedBlackTree::remove` leaves child checked
pointers in a detached timer node. Destroying another timer can leave those
pointers referring to a zeroed dead object, and later destruction of the old
node aborts. [WebKit `a10b7f5`](https://github.com/petitstrawberry/WebKit/commit/a10b7f5ed842d2fe72675c1725a0781c6bb0e25d)
resets detached node links after balancing the tree. The tests cover one- and
two-child nodes, destruction after former children, reinsertion and sorted
iteration, with both ordinary and thread-safe checked pointers. The native
Vulkan probe runs the actual thread-safe node lifetime regression before its
GPU contracts. This watch-page run did not pass playback; verification of the
new tree fix is in progress.


With WebKit `b5cc24f1` (the timer fix plus the probe's private WTF include
root), the release HVF guest passed the timer lifetime regression and all three
native Ganesh/YUV cycles. The actual supplied YouTube watch URL then passed the
video-page probe: 854×480 native video, `currentTime=10.06`, 39 decoded frames,
38 presented frames, no reported drops, and 23,828 changed video-region pixels.
This probe reads back GPU images for verification; its frame count measures the
probe's sampled presentation, not normal GUI playback throughput. The diagnostic
subsequently stalled during owner teardown and was interrupted through its own
foreground process group. Normal GUI playback is visibly slow and remains the
active performance issue.

The next implementation removes the intermediate SgfxCanvas pass for completed
WebKit GPU pages, preserves reusable external BGRA image imports while their
application texture owners remain alive, and routes external-image state updates
through paint invalidation. WebKit exposes a stable allocation identity separately
from its frame lease generation. Myrica caches at most two imported texture
owners; the cache never retains frame leases, and unused exported capabilities
are closed. WebKit also tracks pending paint damage per GPU allocation so a
reused image accumulates changes missed while leased. Resize and layout retain
full-paint behavior. Native release compilation and two-image damage/readback
verification of this revision are in progress.


The two-image GPU contract passed on the release HVF guest using Myrica
`68160392` / WebKit `3bcb35c5` / ScarletUI main `fcea172b`. Allocation IDs were
A=1 and B=2. Holding both leases blocked production; after returning A its next
frame reused A's ID, accumulated two distant DOM background changes, and kept
leased B unchanged. That paint covered 15,360 physical pixels versus a 71,680
pixel viewport. Released generations returned stale/zero identity, and resize
allocated a new identity while preserving B. TLS and three native Ganesh/YUV
cycles also passed. In the user's actual YouTube GUI trial, load and choppy
playback persisted; these source changes alone did not meet the performance
goal.

Further changes decouple the native decode worker from an empty-display-queue
requirement while retaining the three-frame bound. Playback keeps the newest
due output and future frames; paused preview and acquired leases are preserved.
Audio receives low-water priority and otherwise shares decode work fairly.
Twenty-five tests against the real codec dependency and an independently
encoded fMP4 fixture passed. The real supplied YouTube URL subsequently played
with native `/dev/video0` NV12 output, but presentation still fell behind.

WebKit `d27f72c8` uses linked AArch64 assembly LLInt with C-loop and
JIT/DFG/FTL/Yarr JIT disabled. The producer explicitly selects C-loop OFF.
The initial `385a97db` binary exposed a missing port diagnostic counter;
`d27f72c8` fixes its real execution-entry instrumentation. Its release guest
passed TLS, three Ganesh/YUV cycles, the two-image DOM/CSS/JS/input contract,
and the full HTTP fixture including real call/arity/exception/Promise/timer/
DOM/typed-array/allocation-root checks, infinite-script interruption, and owner
shutdown. The ELF has only the expected dynamic-loader imports and weak
`environ`.

The same revision loaded the supplied YouTube watch URL, presented changing
video pixels, reached 10.08 seconds (304 decoded, 226 dropped, 75 presented),
and completed native owner shutdown. These are diagnostic readback results,
not normal GUI FPS; performance remains inadequate. An actual `LINE_LIST`
graphics-pipeline rejection also appeared and is being implemented in SGFX.

SGFX `e4240625` changes Vulkan fence waits from 50-microsecond sleep polling to
the existing completion condition variable. It preserves selected-fence
all/any predicates, immediate/finite/infinite timeout behavior, and GPU
completion synchronization; completion, acquire, failure, loss and destruction
notify waiters. An independent lock-order/lost-wake audit passed, as did 104
unit tests, eight real Metal draw/readback contracts, and native AArch64
compilation. Its first normal rootfs build with Myrica `ddf5b427` and WebKit
`1124361a` completed. Guest performance of this SGFX revision is pending the next normal image.
The live validation image still contains the previous `2bd` driver.

The flattened GPU page renderer still rebuilds page drawing on the CPU.
WebKit `33dead0b` adds the retained `GraphicsLayerCoordinated` /
`CoordinatedPlatformLayer` / `SkiaCompositingLayer` path using the same native
Ganesh/Vulkan owner context. Native video supplies retained GPU images through
the formal coordinated media proxy, with callbacks after successful submit.
Actual tile-raster counters are separate from final page-image compositions.
Native object compilation passed with the feature both ON and OFF; Myrica
`889946de` selects ON for Vulkan through its ordinary producer and normal
bundle. Full release compilation/link passed, including the teardown and
submit-retry fixes. The callback regression remains above the standalone
Skia/Vulkan library and uses the real GPU image/buffer implementation.
Actual guest tile/video regression is pending the updated normal image.

SGFX `125279dd` additionally supports one-pixel LINE_LIST and LINE_STRIP from
Vulkan through IR to real native VirGL primitive tokens 1/3. Existing IR tags
0/1/2 are unchanged; the new topologies use 3/4. Core 27, native VirGL 97, and
Vulkan 104 release unit tests passed. Actual Metal readback checked all 64 RGBA
pixels for nonindexed/indexed line lists and a nonindexed strip; indexed strips
remain explicitly rejected on the WGPU adapter to preserve restart semantics.
Native AArch64 release checking passed. The normal SGFX producer and ICD pins
select this revision; real guest YouTube verification is next.

The normal release image with Myrica `889946de` / WebKit `33dead0b` /
ScarletUI main `fcea172b` / SGFX `125279dd` booted under HVF. Actual native
GPU tests passed three device/YUV/callback-retry cycles and retained
transform/opacity/rounded-clip/mask tests. The HTTP media suite passed all seven
normal/MSE/seek variants. Its static-background video contract observed zero
tile-raster delta, seven page compositions, six frame callbacks and 12,288
changed video pixels. These functional contracts did not establish usable
YouTube performance: the user's real page remained very heavy with a white
video region and many missing thumbnails.

The live GUI journal then revealed native decoder recreation approximately
five times per second, with each instance mapping an 8MiB input and 16MiB output
buffer. Myrica `142b721d` retains identical decoder configuration and queued
samples across repeated init segments; a real fMP4 demux/ABI regression repeated
the init 50 times without changing generation or accepted sample bytes. Actual
recreation logs now identify session, track, generation and configure/flush/seek
reason. Real-page diagnosis of that revision is in progress.

WebKit `a169eb4e` also enables formal composited overflow scrolling, updates
Scarlet's same-owner scroll geometry through ordinary setters, and cancels
stale pending-tile flags for empty coverage. On the native guest the 16px cached
scroll now has zero tile-raster delta. The old far-scroll test at 1100px also
returned correct pixels without new raster; its prefetch boundary is being
checked before moving the fixture genuinely outside existing coverage.

The subsequent native `142b721d` / `a169eb4e` diagnostic reproduced a lower
level video-queue failure on the user's watch page. Newly configured decoder
sessions (generation 1, 854x480) failed with `virtio-video async RESOURCE_QUEUE
failed`; the host reported input queued without an output resource. The guest
then stopped in the kernel panic handler. Read-only QMP inspection and the
matching release ELF located the allocation failure at
`VirtioVideoDevice::complete_pending_used_descriptor` -> descriptor free-list
growth. The free list contained 67,108,864 entries for a 256-descriptor queue;
descriptor 0 retained NEXT with next=0. Its next growth attempted a 1 GiB
allocation. This is a confirmed unbounded descriptor-reclamation failure,
separate from the remaining page-rendering performance investigation.

The video driver's synchronous completion ownership, timeout descriptor reuse,
and session-slot reservation are being corrected together with bounded,
allocation-aware descriptor reclamation. Native runtime validation of those
changes is pending. Myrica `3b04bd7a` includes actual HTTP PNG/JPEG/WebP GPU-pixel
checks, an uncached 10,316px scroll, and pending-coverage cancellation/restoration
checks; native compilation passed and guest execution is pending recovery of
the validation VM.

Scarlet `2a3c6196f` now bounds chain reclamation, clears retired descriptors and
rejects duplicate pool insertion. `e9aa3a8c5` registers synchronous-command
ownership before publication, retains device-owned pages after timeout, reserves
session slots atomically, and retries orphan retirement after late completions
under a single reaper claim. Per-session operations and teardown are serialized.
The host daemon applies release/acquire ordering at the shared-ring indices.
Actual-source queue regressions passed 13 tests and command/retirement state
regressions passed three tests. Native AArch64 release lib and test compilation
passed. Ten real VideoToolbox daemon tests passed, including 12 frames on each
of two streams with pixel, dimensions and timestamp checks while teardown of
one stream was interleaved between the other stream's output/input commands.
These are driver/daemon checks; the updated whole guest has not yet run them.

The updated normal release image booted under HVF with those video fixes and
the frontend operation-lifetime guard (`3b0f8a268`). Actual GPU validation
passed TLS, three Ganesh/YUV/feedback cycles, retained clipping/group opacity,
immutable image leases, zero-raster cached scrolling, new raster after a
10,316px scroll, and pending coverage removal/restoration. The HTTP image suite
passed PNG/JPEG/WebP, downscaling, asynchronous load/decode, transformed image
layers and cached scroll-return pixels. Cookie/blob/iframe/frameset checks also
passed. The following View Transition assertion assumed compositor OFF;
Myrica `bef7fb3` makes ON/OFF expectations explicit without weakening callback,
Promise or GPU pixel assertions.

The same YouTube watch page then decoded four native 854x480 NV12 frames with
no RESOURCE_QUEUE error or kernel panic. Playback nevertheless stalled with
three queued decoded frames, zero presented frames and media time zero.
The owner showed long tick and GPU submission stalls. Diagnosis now targets
the first-frame due-time/play-state/audio-clock gate; these results do not
establish usable YouTube playback. Repeated running-HVF register dumps also
blocked the QEMU monitor in `hmp_info_registers` -> `do_run_on_cpu`; this
validation interruption is being investigated separately from the guest.

After recovering the validation VM, SGFX `bbbe0989`'s real native
`dynamic_smoke --copy-only` passed all 4,096 BGRA pixels for each of ten
changing vertex-buffer updates at offset 96, including two passes, nonzero
first vertex and reuse beyond all four upload arenas. This checks native VBO
copy results through drawing; it does not read arbitrary packed source bytes
directly. The earlier offset-64 fixture was rejected before submission because
64 is not divisible by its 12-byte vertex stride. Consequently the Apple
`gldCopyBufferSubData` warning alone does not justify a copy fallback patch.

The updated video queue/session implementation subsequently passed all seven
native HTTP video fixtures (normal, main/high profile, sniff/reload and both
MSE sizes), audio pause/seek/resume/end checks and owner shutdown. Static
retained video changed 12,288 pixels over seven compositions with zero new
tile rasters. These small fixtures establish functional contracts, not watch
page throughput. The YouTube reproduction still stalled or advanced to the
next playlist item before presenting video. Fork `e5b0ff9e` adds opt-in DOM
video state, parser/EOF lifecycle and separate run-loop/update/finalize/scene
timings so that these cases can be distinguished on the actual page.

Scarlet `153ba8611` and SGFX `b7a17c6e` add capability-gated native R8/RG8
storage and typed uploads/readback, retaining BGRA emulation on older kernels.
For the observed 854x480 NV12 frame, this removes expansion from 614,880 to
2,049,600 bytes and reduces whole-row inline upload packets from 33 to 11.
The old packets already shared one SGFX receipt; these counts are not separate
Vulkan submits. Backend unit tests (102), native compilation and an independent
ABI/format/stride/lifetime review passed. Native odd-width texture, sampling,
blend and readback execution was the next validation step.

The actual new-kernel guest exposed a fixed-function BGRA-only render-target
guard after R8 upload/copy/sampling and mip checks had passed. SGFX `12c280bf`
extends fixed pipeline lowering to narrow targets with the same missing-alpha
and color-mask rules as programmable pipelines. Its 103 unit tests and native
compilation passed. The corrected dynamic driver then passed the complete
`--narrow-only` contract for both R8 and RG8: padded odd-width upload, every
readback pixel, partial copy/clear, sampling/swizzle, mip linear blit and
destination-alpha blending. The ten-update native vertex-copy contract also
passed with this driver. The full native WebKit TLS/Ganesh/YUV/retained-scene
contract passed on kernel `153ba8611`.

With e5b0ff9e, native narrow storage and the corrected video queues, the real
canonical YouTube watch page advanced ten seconds: 302 decoded frames, 268
dropped, 31 presented, and 423,504 changed video-region GPU pixels. Its DOM
buffered range grew to [0.033333,13.966666], with no media error. This establishes
actual playback but exposes severe frame dropping; diagnostic GPU readback
adds work compared with ordinary GUI presentation. Separate timing counters
showed GPU submission/completion waits taking 0.6–0.87 seconds in several
one-second intervals, alongside significant run-loop time. The synchronous
Ganesh submit currently blocks the WebCore owner until GPU completion.

The corresponding normal GUI (2196x1282, without diagnostic image readback)
settled at 15–19 page submissions per second, then exited on a preserved
ScarletUI diagnostic: `ImportExternalImage: Dynamic(Status(2))`. A subsequent
application failed at the same import in three seconds, and another reported
vertex-specialization device-memory errors. These are remaining failures.
SGFX `a2a40098` fixes a separate source-confirmed lifetime defect: IR mapping
created a distinct sampler view, but unmapping never destroyed that view.
The fix retires the owned view after accepted work, preserves borrowed surface
ownership and adds import-stage/physical-image failure diagnostics. Its 104
unit tests and native compilation passed; long-running GUI validation is
still required to determine whether other exhaustion paths remain.

WebKit `13b6c00a` adds pending/ready/leased output states and Ganesh async
completion polling on the owner. An incomplete image cannot be published or
reused, and resize/navigation/teardown drain pending work. The native syntax
checks and independent lifetime review passed. Myrica `d5e46103` adds genuine
pending-image/resize/immutable-lease/destruction checks and initial EMPTY
pumping to its GPU probes. Baseline JIT remains unvalidated at runtime: the
native build exposed a DFG-disabled header dependency, fixed in `eda3071d`,
and the embedding guard was updated with the async change. Full integration
is being built through the ordinary release bundle producer.

Independent review found that a partially failed sampler-view cleanup could
leave the same session usable. SGFX `ffc70472` preserves the existing failed
submission flag after backend retirement errors and rejects subsequent import,
draw and repeated cleanup before GPU access. The prefix-applied failure
regressions, 106 backend tests, native compilation and follow-up review passed.
Baseline-only JSC also required direct jump-table definitions and an inline
cache Variant that omits its DFG alternative when DFG is disabled. Fork
`4dd13549` includes those fixes; all 52 native JavaScriptCoreJIT compile entries
passed with actual generated PCHs in validation.

The normal release disk subsequently booted Myrica `a0e536cc`, WebKit
`ee71c5a0`, ScarletUI `bf024ea7` and SGFX `ffc70472` under HVF. The actual
async GPU probe passed pending-image capacity/generation gating, immutable
leases, pending resize/coverage/destruction drains, retained-scene scrolling,
TLS, and all eight padded I420/NV12 color-conversion cases. The normal driver
also passed both native narrow-format contracts and ten changing vertex-copy
updates. JavaScriptCore generated baseline machine code for actual fixture
functions; the full JavaScript semantics suite and interruption of an infinite
script passed with JIT enabled. HTTP image/storage/frame/View Transition,
idle/pointer retention, damage and resize checks passed after fixture
`38af673` correctly awaited a fresh GPU revision and its expected pixels.

The ordinary GUI watch page decoded approximately 30 frames per second and
advanced past 76 seconds, but composed only 13–18 frames per second. Async
submission cost was only a few milliseconds per one-second interval; reported
completion latency includes delayed owner polling. Source inspection found
that the worker charged the 16.667ms frame deadline both when starting an
async image and when collecting it. Completion collection and submission
pacing are being separated without increasing WebCore timer/layout polling.

A user resize reproduced an exact import failure: existing physical BGRA
image 807 (2196x1282) was reattached to the same GPU context while the new UI
target was 1304x760. Sampling the old image during resize is valid. The UI
retains the previous mapped session until SWS releases its committed frame,
but the kernel rejected a second attachment of the same image capability.
Context-local attachment reference counting and mapped-session teardown are
being corrected together; dropping only the image handle does not release
the context's retained attachment/backing reference.

Scarlet `4b3d5f401` formally reference-counts repeated image attachments by
backend image Arc identity. It preserves backend validation for distinct image
objects, uses checked counts, detaches physically only on the last reference,
and rolls back the newly acquired reference when publishing the ioctl response
fails. Backend detach failures retain a retryable final reference. Eight
actual-source host regressions, native AArch64 release library/tests compilation,
and independent review passed. The normal BSP release build also succeeded.
Myrica `abb45aa0` separates submission pacing from pending/ready GPU collection;
its nine worker regressions passed, including delayed completion, lease
backpressure, idle polling and old-viewport rejection during resize. WebKit
`55ca4225` supplies a pure owner-thread state getter so collection cannot start
a new paint accidentally. The normal release rootfs producer built this pair.

A temporary guest execution of the newly produced ordinary Myrica binary
(`abb45aa0` / `55ca4225`, no diagnostic image readback) on the still-running
old kernel/driver improved steady 2196x1282 watch-page composition from about
13–18 to 22–30 completions per second while native decoding remained about
30 fps. The video clock advanced beyond 33 seconds. Some intervals still had
approximately 0.4s of owner run-loop work, and initial cold page loading had
multi-second JavaScript/layout stalls. This comparison validates the pacing
change, while the attachment fix and complete session cleanup still require
the replacement normal image and resize/long-run execution.

SGFX `e36b4903` completes normal mapped-session teardown, including imports,
private IR objects/textures/buffers/upload arenas, mapped target surfaces and
backings, and each queue's private composition VBO. Accepted work is awaited
before retirement; uncertain cleanup poisons the shared context and prevents
subsequent unsafe cleanup/submission. Partial session construction shares the
same cleanup path. Backend tests (107), real IR fixture tests (3), native
release plugin/example/legacy compilation and independent review passed. The
new `--import-overlap-only` fixture exercises six resize generations, accepted
pending work across old-session Drop, new-session drawing, last-release
reimport and producer destruction while a lease remains. Its real guest run
is the next gate. The normal rootfs was rebuilt with this driver and Vulkan
implementation. Both pacing comparison runs used two vCPUs; final replacement
VM validation uses the runner's normal four-vCPU default with HVF.

The replacement normal disk booted kernel `4b3d5f401`, Myrica `abb45aa0`,
WebKit `55ca4225`, SGFX `e36b4903` and ScarletUI `bf024ea7` with four vCPUs.
The real `--import-overlap-only` contract passed all six generations, including
old accepted-work completion during Drop, new-session pixels after old Drop,
last-release/reimport and an image lease surviving producer destruction:
`SGFX_IMPORT_OVERLAP_ONLY_OK generations=6 old_drop_new_draw=6 producer_drop_lease=1`.

Myrica `94c6828` additionally publishes completed images before a due owner
JavaScript tick; a gated regression proves the UI mailbox already holds the
completed image while the subsequent tick is deliberately blocked. All ten
worker regressions passed. A separate strict media fixture exposed MP4 edit
lists being parsed but ignored in Symphonia: MAIN/HIGH native packet timestamps
were 250ms later than FFmpeg's edited timeline. Fork `95d109e5` applies the
edit consistently to signed PTS/DTS, duration and seek, preserving encoded
preroll. Six unit tests and actual MAIN/HIGH/leading-empty files (72 packets,
12 seeks, independently compared with FFmpeg) passed. Scarlet `7ea687d91`
pins the fix and Myrica `99a87e87` aligns its three native media dependencies
on that source. Complex/non-unit edits are rejected; Scarlet Sample does not
yet propagate packet trim metadata for arbitrary edit-window crop/padding.
These additions are being built into the normal release bundle.

The current normal driver also passed the complete `/bin/myrica-vulkan-probe`
contract: TLS, three Ganesh lifecycles, all eight YUV variants, feedback,
pending capacity/generation gating, immutable two-image leases, retained
compositing/scrolling, resize/coverage and owner-destruction drains. Native
Myrica `99a87e87` then passed all seven strict HTTP video fixtures, including
MAIN/HIGH reordered original PTS, pause/seek/resume/end, sniff/reload and both
MSE sizes. Retained video changed 12,288 pixels over seven compositions with
zero tile rasters. The full HTTP/DOM/CSS/image/storage/frame/View Transition,
JavaScript semantics and infinite-script interruption suite also passed,
including clean owner destruction and join. Final ordinary GUI throughput
and live resize remain the final image check.
