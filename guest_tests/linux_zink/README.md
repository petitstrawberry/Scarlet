# Current OpenTTD integration checkpoint

The historical investigation below records intermediate failures. The current
coordinated branches run OpenTTD 15.3 through SDL 2.30.0 Wayland/EGL -> Mesa
25.0.7 Zink/Kopper -> SGFX Vulkan -> Scarlet VirGL on ARM64 QEMU/HVF.
Clear, triangle, RGBA/R8/RG8/1D palette and partial-readback probes passed.
Window launch, keyboard/mouse, map scrolling, move/resize, maximize/restore,
ScarletUI SSD, rounded corners, and close were verified with actual guest captures.

OpenTTD fullscreen still fails the SGFX viewport bounds rule; the independent
SHM fixture passes fullscreen. Physical hardware is unverified. Capability
warnings remain, and no Vulkan/Zink conformance claim follows from gameplay.
See `tools/graphics/openttd/README.md` for fixed fork commits, reproduction,
source provenance, and the final screenshot.

# Historical Zink integration baseline

Zink is a useful application workload for improving the SGFX Vulkan ICD:
it exercises shader translation, graphics state, descriptors, uploads and
GPU completion together. The first target is a Linux offscreen EGL device/pbuffer context
with a clear, triangle and textured quad verified by pixel readback. Window
presentation and Wine's OpenGL renderer follow after that target works.

The investigation pins **Mesa 25.0.7**, rather than changing requirements with
Mesa main. Its official source archive is
`https://archive.mesa3d.org/mesa-25.0.7.tar.xz`, SHA256
`592272df3cf01e85e7db300c449df5061092574d099da275d19e97ef0510f8a6`.
The source references are `docs/drivers/zink.rst`,
`src/gallium/drivers/zink/VP_ZINK_requirements.json` and `zink_screen.c`.
The archive was downloaded and verified during the initial investigation.

`requirements.c` inventories 22 extension/feature checks from that source's
Vulkan 1.0 GL 2.1 baseline. It uses the regular Vulkan loader, prints the
selected devices, counts core promotions, and queries extension features
only when the device supports the corresponding extension or core version.
It returns **2 for missing capabilities**, **1 for probe failure**, and **0
when the inventoried checks pass**. The inventory is not the complete Vulkan
Profile checker: format/limit checks and the alternate line-rasterization
requirements are not covered. Passing it does not establish correct rendering.

Build/install with the ordinary graphics producer, then run in guest Bash:

```sh
tools/graphics/build-linux-vulkan.sh /path/to/sgfx
# Build the Scarlet image on the host after installing the overlay.
/opt/sgfx-vulkan-tests/run.sh zink-capabilities
```

The initial HVF guest (8 vCPUs, 8 GiB RAM) identified
`SGFX Vulkan (Scarlet VirGL GPU 0)`, Vulkan 1.0.0, and reported all 22 checks
missing. This is an inventory of advertised support, not proof that the GPU
cannot execute each feature. Extension and feature checks overlap.
The capture is `artifacts/cave-story/zink-requirements-baseline.log`.

The next implementation priorities are:

1. Timeline semaphores, feature queries and device feature-chain handling.
   Mesa 25.0.7 aborts screen creation when neither the timeline extension
   nor the Vulkan 1.2 timeline feature is available. Signal values must track
   actual GPU completion, including host waits and queue dependencies.
2. Render-pass/descriptor APIs and memory layouts needed by actual Zink
   initialization and shaders. Implement supported operations before
   advertising their extensions or features.
3. GPU-verified clear, triangle, texture and blending readback tests. Record
   the GL renderer, selected Vulkan device and pixel results; software Mesa
   rendering is not a successful SGFX/Zink run.
4. EGL/Wayland presentation. The existing private Scarlet GPU-image protocol
   is not dma-buf or a Mesa EGL buffer transport. Offscreen success does not
   establish native window presentation. Zink's DRM route additionally
   requires external-memory FD support.
5. Wine's existing OpenGL path and, finally, the original Cave Story binary.

Do not suppress Zink's requirement warnings or force unsupported feature
flags to obtain a higher reported GL version. Keep capability inspection,
initialization success, pixel correctness and presentation as separate results.

## Implemented and verified in the first integration stage

The ICD now exposes `VK_KHR_timeline_semaphore`. `vkQueueSubmit` copies its
packets and returns without waiting for a future host signal. A FIFO queue
worker preserves packet boundaries; the completion worker advances timeline
values only after actual SGFX GPU receipts retire. Host counter, signal,
ANY/ALL waits, timeouts and binary acquire/present synchronization remain
separate. Admission is bounded to 4096 pending packets and 16384 referenced
objects per submit call. Queued work and accepted GPU work have separate counters so resource
cleanup does not block the resource worker on an unsatisfied future host wait.

The HVF guest passed `timeline.c`: submission with an unsatisfied wait returned
in 0.000 s; the counter remained zero; a zero-timeout wait timed out; a host
signal unblocked the packet. A real GPU image clear/copy retired before its
timeline signal, and every readback pixel matched. Buffer Fill/Update readback
also matched every word, including record-time Update snapshots. Two submit packets then signaled/consumed one
binary semaphore and retired their fence. `vulkan-offscreen --regional` also
verified a scaled partial blit between different-sized images, including the
pixels outside the destination rectangle. The capability inventory decreased
from 22 to **20 missing checks**, because extension/feature checks overlap.

RenderPass2 lowering is implemented and unit-tested, but its extension is **not
advertised** yet: its maintenance2/multiview dependencies and GPU integration
must be completed first. API presence alone is not the capability claim.

## Build and run the executable GL benchmark

```sh
tools/graphics/build-linux-vulkan.sh /path/to/sgfx
tools/graphics/build-linux-zink.sh
# Rebuild the Scarlet image after installing the overlay.
/opt/sgfx-vulkan-tests/run.sh zink-timeline
/opt/sgfx-vulkan-tests/run.sh zink-viewport
/opt/sgfx-vulkan-tests/run.sh zink-capabilities
/opt/sgfx-vulkan-tests/run.sh zink
```

The producer builds the checked Mesa archive in Docker for ARM64, installs it
under `/systems/linux-aarch64/opt/sgfx-zink` in the experimental overlay, and
adds `zink-egl` plus its scoped environment launcher. It builds Mesa with debug
messages and optimized code, so startup requirement failures are visible.
A software EGL device supplies memory-backed pbuffer transport without DRM;
Zink independently selects the SGFX Vulkan GPU. The default probe requires
`GL_RENDERER` to contain both `zink` and `SGFX`, then verifies a red clear,
a green triangle over it, and a four-color 2x2 texture on a quad.

Softpipe is included to expose this EGL transport and provide an explicit
`zink-egl --reference` harness check. The container's reference check passed
all three pixel tests. Reference success is **not** SGFX/Zink success; the
normal launcher has no reference mode and rejects a softpipe renderer.

The actual guest Mesa startup currently reports:

```text
ZINK: VK_KHR_create_renderpass2 required!
ZINK: failed to detect features
```

Mesa can subsequently fall back to softpipe; the probe rejects that fallback.
The native VirGL path now advertises `VK_KHR_maintenance1`. Its signed viewport
is retained through IR bounds validation and the VirGL transform. The WGPU path
rejects signed/zero-height viewports and does not advertise this extension.
Transfer format bits follow actual image-format/usage queries; 3D image formats
remain unsupported. Command-pool trim shrinks unused registry capacity without
resetting allocated buffers or changing accepted submission snapshots.

`viewport.c` passed on HVF (8 CPUs, 8 GiB) with the Scarlet VirGL GPU: its 496
triangle pixels matched an exact vertical mirror, zero-height drawing retained
only the blue clear, and all three submissions survived trimming their executable
command buffer. The test also found and fixed GENERAL-layout attachment access
being incorrectly lowered as sampled access. Timeline GPU clear/readback and
regional blit passed with the same ICD. The inventory is now **19 missing checks**;
Mesa's next startup error is `VK_KHR_create_renderpass2 required!`.

The next blockers in Mesa's extension generation are create_renderpass2,
imageless_framebuffer and descriptor_update_template. RenderPass2 needs its
maintenance2/multiview extension dependencies implemented as well as GPU checks.
Do not just advertise these extensions to advance initialization.
No Zink GPU GL pixels, EGL window presentation or Wine OpenGL gameplay have
been verified yet. Captures: `artifacts/cave-story/timeline-gpu.log`,
`vulkan-region-gpu.log`, `zink-requirements-timeline.log`,
`zink-egl-diagnostic.log`, `maintenance1-gpu.log` and `zink-reference.log`.
`SGFX_VULKAN_TRACE=1` enables error diagnostics for rejected IR, backend failures
and asynchronous queue packets during integration debugging.

The release image was rebuilt after installing this stage in the experimental
overlay. A fresh test root copied from that generated image passed viewport,
timeline, all Vulkan kernel-I/O probes, full clear readback, regional blit and
two-generation Wayland presentation under HVF. The test used the generated
ESP/rootfs as separate drives (only its test boot root-device argument and SSH
public key differed from the distributed image). The embedded viewport probe's
SHA256 matched the producer output. The complete capture, including the still
failing Zink startup, is `artifacts/cave-story/maintenance1-final-gpu.log`.

## Wayland window validation

`tools/graphics/build-linux-zink.sh` also builds a Wayland EGL/Kopper runtime,
`zink-window`, and `zink-window-run`. The window probe requires an actual
`zink` + `SGFX` renderer, verifies a GPU clear by pixel readback, and reports
whether the first swap and later frames returned. Key and mouse events are
logged; these logs alone are not proof of visible presentation, so capture the
test window separately.

```sh
XDG_RUNTIME_DIR=/tmp WAYLAND_DISPLAY=wayland-test \
  /opt/sgfx-vulkan-tests/zink-window-run
# The same wrapper can run an application with process-local configuration:
/opt/sgfx-vulkan-tests/zink-window-run openttd -v sdl-opengl
```

Mesa 25.0.7's Wayland initializer normally requires a DRM node even for Zink.
The shared `petitstrawberry/mesa` fork extends its existing `LIBGL_KOPPER_DRI2=1` opt-in to Wayland,
letting Vulkan WSI own buffer transport. SGFX's Vulkan driver uses its private
`wp_scarlet_sgfx_v1` Wayland protocol; no dma-buf emulation is introduced.
The generic EGL device bookkeeping can expose `EGL_MESA_device_software` in
this mode, as in upstream X11's no-DRM Kopper path. That is not proof of CPU
rendering: record the GL renderer and Vulkan physical device independently.
Never enable `LIBGL_ALWAYS_SOFTWARE` or override the GL version for this test.

`wayland-shm-resize` is a GPU-independent regression test for the SDL cursor
bootstrap. It creates a pool from a Linux memfd, grows the file and pool, then
creates a buffer beyond the original extent. It repeats this with an unlinked
regular file. Run against the test bridge with the same `WAYLAND_DISPLAY`;
both cases must survive a server roundtrip. A backing-file mapping failure is
reported separately from successful pool growth.
