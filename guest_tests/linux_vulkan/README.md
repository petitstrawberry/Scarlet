# Linux and Win64 Vulkan regression probes

Build and install the ICD, its SWS C library, and these probes into the
experimental overlay on macOS or Linux with Docker:

```sh
tools/graphics/build-linux-vulkan.sh /path/to/sgfx
direnv exec . cargo scarlet image --project projects/aarch64-limine-full --release
```

The SGFX source must include `scarlet-wsi`, `VK_KHR_wayland_surface`, the
Scarlet SGFX Wayland protocol, and GPU `vkCmdClearColorImage` support. The
producer checks that its protocol XML matches the bridge's contract. It uses
an ARM64 Ubuntu 24.04 container, matching aarch64 glibc binaries rather than
host macOS libraries. Build outputs and installed experimental files are
ignored artifacts; the sources and producer live in Git.
The companion SGFX implementation was tested at commit `6e37c37` on branch
`feat/linux-wine-vulkan`.

In Scarlet bash, with the VirGL GPU and both SWS and wayland-bridge running:

```sh
/opt/sgfx-vulkan-tests/run.sh linux
WINEPREFIX=/var/tmp/wine-vulkan-tests /opt/sgfx-vulkan-tests/run.sh wine
```

`offscreen.c` uses the ordinary Linux `libvulkan.so.1` or Windows
`vulkan-1.dll`. It clears a 64x64 BGRA image on the GPU, copies it to a
host-visible buffer and checks every pixel. `present.c` uses a standard
Wayland surface or a Win32 HWND. It presents 20 frames at each of two sizes,
recreates the swapchain with `oldSwapchain`, and checks release/reuse beyond
the image count. The Linux window deliberately combines a SHM root with a
desynchronized GPU child, covering the composition path used by Wine.

`kernel_io.c` checks `getcpu` outputs and bad pointers, process-owned POSIX byte-range locks, splitting and lock
conversion, native image-capability export as a CLOEXEC Linux descriptor,
SCM_RIGHTS ownership after the original handles close, and independent
relative Unix socket names with unlink/rebind. These are prerequisites for
Wine startup and Wayland GPU buffers.

The Win64 executables are PE32+ x86-64 programs built with MinGW. Wine runs
them through Box64; Box64 wraps the ARM64 Vulkan loader, which discovers SGFX
using `/usr/share/vulkan/icd.d/sgfx.json`. No `VK_DRIVER_FILES` override or
private Vulkan loader is used. A fresh Wine prefix still runs Wine's normal
initial setup. Optional pointer confinement and relative motion remain
unsupported by the bridge.

The ICD is a non-conformant Vulkan 1.0 development subset. These probes do
not establish Vulkan 1.1/1.3, DXVK, Direct3D, or general Windows game support.
Only BGRA8, opaque FIFO Wayland presentation is currently exposed; render
completion precedes compositor sampling, and reuse waits for wl_buffer.release.
The private image protocol is not dma-buf, EGL, or OpenGL support. Contended
blocking POSIX lock acquisition remains unsupported.

On the tested Scarlet runtime, the Win64 probes reach their GPU success checks,
but the Wine Unix launcher can still receive SIGKILL during exit and return 137.
`run.sh` reports and preserves that failure; GPU output alone is not a successful
process-lifecycle test. This exit issue remains unresolved. Steam has not been
tested.
