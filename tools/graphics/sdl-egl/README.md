# SDL2 EGL with legacy OpenGL contexts

Build using `tools/graphics/build-linux-sdl-egl.sh`. The dedicated container
checks out the fixed SDL 2.30.0 compatibility commit from
`petitstrawberry/SDL` (`codex/egl-legacy-context-2.30`), and places a private runtime in `artifacts/linux-sdl-egl-build/runtime`.
It does not install or replace the host or guest system SDL.

SDL 2.30.0's EGL context creation probes desktop surfaceless support using
`glGetIntegerv(GL_MAJOR_VERSION)`, which is invalid below GL 3.0. The query
leaves `GL_INVALID_ENUM` in a real Zink GL 2.1 context. OpenTTD's later texture
check can consequently report a texture creation failure despite valid texture
operations. The patch reads `GL_VERSION` and parses its major instead, retaining
the same major >= 3 rule. No GL error is consumed and no advertised version or
extension is overridden.

Select the resulting `lib/libSDL2-2.0.so.0` with the dedicated test process's
`LD_LIBRARY_PATH`. Keep the Mesa and SGFX Vulkan library directories as well.
For example, prepend `/opt/sgfx-sdl/lib` to the environment used by
`guest_tests/linux_zink/run-window.sh` or the explicit OpenTTD `sdl-opengl`
driver. This build disables libdecor plugins; the Scarlet Wayland bridge
provides ScarletUI server-side decorations through xdg-decoration. Other SDL
backends are detected normally.

The same correction is forward-ported onto the existing SDL 2.32.10 native
Scarlet branch as `codex/egl-legacy-context`. That branch retains the SWS backend,
but native SDL 2.32.10 gameplay was not verified by this Linux ABI integration.

`guest_tests/linux_zink/window.c` checks for GL errors immediately after
context creation, before its independent Zink/SGFX renderer, full-frame GPU
readback, and Wayland presentation checks. This distinguishes the SDL bug from
subsequent Vulkan rendering failures.
