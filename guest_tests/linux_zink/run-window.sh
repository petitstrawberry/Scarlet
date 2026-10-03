#!/bin/sh
set -eu
runtime="${SGFX_ZINK_RUNTIME:-/opt/sgfx-zink}"
export LD_LIBRARY_PATH="$runtime/lib/aarch64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export __EGL_VENDOR_LIBRARY_FILENAMES="$runtime/share/glvnd/egl_vendor.d/50_mesa.json"
export MESA_LOADER_DRIVER_OVERRIDE=zink
# Let Vulkan WSI own buffer allocation and transport. Scarlet SGFX exports its
# own Wayland buffer protocol and does not expose Linux DRM/dma-buf nodes.
export LIBGL_KOPPER_DRI2=1
export SDL_VIDEODRIVER=wayland
export VK_DRIVER_FILES="${SGFX_VULKAN_ICD:-/usr/share/vulkan/icd.d/sgfx.json}"
export VK_ICD_FILENAMES="$VK_DRIVER_FILES"
unset GALLIUM_DRIVER LIBGL_ALWAYS_SOFTWARE LIBGL_KOPPER_DISABLE MESA_GL_VERSION_OVERRIDE MESA_GLSL_VERSION_OVERRIDE
if [ "$#" -gt 0 ]; then
    exec "$@"
fi
exec "$(dirname "$0")/zink-window"
