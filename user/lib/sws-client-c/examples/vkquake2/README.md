# vkQuake2 SWS platform adapter

This adapter replaces the upstream X11 window and input implementation with SWS
and `VK_KHR_display`. The engine, Vulkan renderer, SPIR-V shaders, and Khronos
Vulkan loader remain upstream code. Link the adapter and SGFX ICD to the same
`libsws_client_c.so` so they share one SWS connection. The initial port uses the
upstream null sound driver.

Tested source baseline:
`kondrak/vkQuake2` commit `6763f207229f97cffabb6fc2da72017a794b139b`.
Build on Linux AArch64 with ordinary Vulkan headers/loader, GCC, Make, Linux input
headers, and X11 headers required by the upstream Vulkan header:

```sh
make -f /path/to/this/Makefile sws-release \
  VKQUAKE2_SRC=/path/to/vkQuake2 SWS_LIB_DIR=/path/to/sws-client-c/target/release
```

The output directory contains `quake2`, `ref_vk.so`, and
`baseq2/gameaarch64.so`. The build uses release optimization and C++17 for the
upstream VMA allocator. The upstream Linux Makefile's C++11 setting otherwise
selects an unsupported allocator that returns null when assertions are disabled.

Build the C SDK and ICD on Linux AArch64 as well. For a musl Rust toolchain,
turn off static CRT linkage so Cargo emits usable Linux shared libraries:

```sh
cd /path/to/Scarlet/user/lib/sws-client-c
RUSTFLAGS='-C target-feature=-crt-static' cargo build --release
cd /path/to/sgfx
RUSTFLAGS='-C target-feature=-crt-static -L native=/path/to/Scarlet/user/lib/sws-client-c/target/release' \
  cargo build --locked --release -p vulkan-sgfx --no-default-features --features scarlet-wsi
```

Use the coordinated SGFX, canonical IR and SDK revisions recorded in
[the graphics execution guide](../../../../../docs/graphics/vulkan-games.md).
The loader is the distribution's existing Khronos library; it is not rebuilt
as part of SGFX. The game renderer links only to its normal Vulkan entrypoints.

Install with the ordinary Make target, using the Linux Environment's backing
directory as `DESTDIR`:

```sh
make -f /path/to/this/Makefile sws-install \
  VKQUAKE2_SRC=/path/to/vkQuake2 \
  DESTDIR=/path/to/project/rootfs/systems/linux-aarch64
```

Supply your own `baseq2` game data under `usr/share/vkquake2/baseq2`. Install the
Linux shared libraries `libsws_client_c.so`, `libvulkan_sgfx.so`, the unmodified
Khronos `libvulkan.so.1`, and a matching `libstdc++.so.6` in the same backing
tree's `usr/lib`. Preserve the Environment's existing musl libc and libgcc.
Install a standard ICD manifest at `usr/share/vulkan/icd.d/sgfx.json` whose
`ICD.library_path` is `/usr/lib/libvulkan_sgfx.so` and `api_version` is `1.0.0`.
The AArch64 full project's normal `rootfs` copy layer includes this tree;
locally staged binaries and game data are ignored by Git.

Linux sees these libraries at `/usr/lib`; Scarlet's default view sees them at
`/systems/linux-aarch64/usr/lib`. Its shell configuration already sets
`LD_LIBRARY_PATH=/usr/lib:/lib`. Neither `LD_PRELOAD` nor a private loader
environment variable is needed. Run from the native Scarlet shell:

```sh
abi-run linux-aarch64 /bin/sh /usr/games/vkquake2 +map demo1 </dev/null
```

Append `+bind f12 screenshot +bind f11 quit` before the redirection to bind
the upstream screenshot and clean-exit commands. F12 writes a TGA under
`/usr/share/vkquake2/baseq2/scrnshot`; F11 shuts down the game normally.

Redirecting stdin gives the game its own nonblocking input descriptor; keyboard
and mouse input arrive through SWS. The launcher disables CD audio, music, and
point particles through ordinary upstream settings. Audio output is not part of
this port.

Runtime dependencies are:

```text
quake2 (Linux/musl application)
  -> dlopen ref_vk.so and baseq2/gameaarch64.so
ref_vk.so
  -> Khronos libvulkan.so.1 -> system ICD manifest -> libvulkan_sgfx.so
  -> libsws_client_c.so (SWS window/input connection)
  -> libstdc++.so.6 (upstream VMA allocator)
libvulkan_sgfx.so
  -> linked SGFX frontend, canonical IR, Naga/TGSI and native VirGL backend
  -> libsws_client_c.so (the same SWS connection, GPU image presentation)
  -> explicit Scarlet native object syscalls -> kernel virtio-gpu
  -> QEMU VirGLRenderer -> host OpenGL driver
```

Ordinary musl, libc and Rust std syscalls remain Linux ABI calls. The C SDK and
native GPU backend use the explicit Scarlet namespace for native object handles.
Use a kernel containing the native namespace, `clock_nanosleep`, pagewise
vectored I/O, mip image controls, in-place `mremap` shrinking, batched AArch64
unmap invalidation, batched private backing reclamation, and unsupported
custom-signal error fixes.
The full project's kernel and module pins select these fixes together.

Actual Scarlet AArch64 QEMU checks establish ordinary-loader device discovery,
60 `VK_KHR_display` presentations with clean shutdown, and all 19 thread-signal
regression checks. The game loads its upstream renderer, creates its Vulkan
resources, loads the game module, and initializes the `demo1` server.
The normal full-project release image also renders the textured `demo1` 3D
world, first-person weapon and HUD through the native VirGL GPU path. Actual
QEMU window captures show different player views after input, and an explicit
keyboard event opens the console and pauses the world. The game also saves its
own 1280x800 GPU-read TGA, and its normal `quit` command returns exit status 0
with no SGFX worker tasks remaining. Combat and sustained FPS have not been
verified. The Linux release ICD now batches consecutive programmable VirGL
draws sharing a pipeline, with bounds on draw count and packet bytes. The
graphics execution guide records its source revision, limited CPU submission
comparisons and three short timedemo measurements. They remain too slow for
normal play and do not establish sustained performance.

Four real 16 MiB `mremap` shrink checks pass on Scarlet after implementing the
game's hunk resize. Four 8 MiB partial-unmap checks preserve retained neighbors
and zeroed replacement pages; their total unmap time drops from 509 ms to 4 ms
after batch physical reclamation. This is an allocator microbenchmark, not a
game FPS measurement. These results do not establish Chromebook/A618 game
compatibility.
