#!/usr/bin/env bash
set -euo pipefail
scarlet_root="$(cd "$(dirname "$0")/../../.." && pwd)"
output="${SCARLET_SDL_OUTPUT:-$scarlet_root/artifacts/linux-sdl-egl-build}"
recipe="$scarlet_root/tools/graphics/sdl-egl"
image="${SCARLET_SDL_IMAGE:-scarlet-linux-sdl-egl-build}"
mkdir -p "$output"
docker build --platform linux/arm64 -t "$image" "$recipe"
docker run --rm --platform linux/arm64 --read-only --tmpfs /tmp \
    --mount "type=bind,source=$recipe,target=/recipe,readonly" \
    --mount "type=bind,source=$output,target=/out" \
    "$image" sh /recipe/build-container.sh
printf 'SDL EGL compatibility runtime: %s/runtime/opt/sgfx-sdl\n' "$output"
