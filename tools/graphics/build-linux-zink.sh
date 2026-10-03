#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/../../.." && pwd)"
output="$task_root/artifacts/linux-zink-build"
recipe="$task_root/tools/graphics/zink"
mkdir -p "$output"
docker build --platform linux/arm64 -t scarlet-linux-vulkan-build -f "$task_root/tools/graphics/vulkan/Dockerfile" "$task_root/tools/graphics/vulkan"
docker build --platform linux/arm64 -t scarlet-linux-zink-build "$recipe"
docker run --rm --platform linux/arm64 \
    -v "$task_root:/scarlet:ro" -v "$output:/out" \
    scarlet-linux-zink-build sh /scarlet/tools/graphics/zink/build-container.sh
destination="$task_root/bundles/experimental/fs/systems/linux-aarch64"
mkdir -p "$destination/opt"
cp -a "$output/runtime/opt/sgfx-zink" "$destination/opt/"
mkdir -p "$destination/opt/sgfx-vulkan-tests"
install -m 755 "$output/runtime/opt/sgfx-vulkan-tests/zink-egl" "$destination/opt/sgfx-vulkan-tests/"
install -m 755 "$output/runtime/opt/sgfx-vulkan-tests/zink-run" "$destination/opt/sgfx-vulkan-tests/"
install -m 755 "$output/runtime/opt/sgfx-vulkan-tests/zink-window" "$destination/opt/sgfx-vulkan-tests/"
install -m 755 "$output/runtime/opt/sgfx-vulkan-tests/zink-window-run" "$destination/opt/sgfx-vulkan-tests/"
install -m 755 "$output/runtime/opt/sgfx-vulkan-tests/wayland-shm-resize" "$destination/opt/sgfx-vulkan-tests/"
install -m 755 "$output/runtime/opt/sgfx-vulkan-tests/wayland-decoration" "$destination/opt/sgfx-vulkan-tests/"
printf 'Installed Mesa 25.0.7 Zink benchmark in %s\n' "$destination"
