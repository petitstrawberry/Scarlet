#!/bin/bash
set -eu
runtime="${SGFX_ZINK_RUNTIME:-/opt/sgfx-zink}"
export LD_LIBRARY_PATH="$runtime/lib/aarch64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export __EGL_VENDOR_LIBRARY_FILENAMES="$runtime/share/glvnd/egl_vendor.d/50_mesa.json"
export MESA_LOADER_DRIVER_OVERRIDE=zink
export VK_ICD_FILENAMES="${SGFX_VULKAN_ICD:-/usr/share/vulkan/icd.d/sgfx.json}"
# A software EGL device supplies pbuffer transport; Vulkan still selects SGFX.
# These overrides would select a CPU Vulkan device or bypass the Zink loader.
unset GALLIUM_DRIVER LIBGL_ALWAYS_SOFTWARE LIBGL_KOPPER_DISABLE MESA_GL_VERSION_OVERRIDE MESA_GLSL_VERSION_OVERRIDE
exec "$(dirname "$0")/zink-egl"
