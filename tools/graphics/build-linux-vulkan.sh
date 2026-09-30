#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ $# != 1 ]]; then
    echo "Usage: $0 <sgfx-source-directory>" >&2
    exit 2
fi
sgfx_source="$(cd "$1" && pwd)"
output="$task_root/artifacts/linux-vulkan-build"
recipe="$task_root/tools/graphics/vulkan"
[[ -f "$sgfx_source/crates/vulkan-sgfx/Cargo.toml" ]] || exit 2
cmp "$recipe/scarlet-sgfx.xml" "$sgfx_source/crates/vulkan-sgfx/protocol/scarlet-sgfx.xml"
mkdir -p "$output"
docker build --platform linux/arm64 -t scarlet-linux-vulkan-build -f "$recipe/Dockerfile" "$recipe"
docker run --rm --platform linux/arm64 \
    -v "$task_root:/scarlet:ro" -v "$sgfx_source:/sgfx:ro" -v "$output:/out" \
    scarlet-linux-vulkan-build sh /scarlet/tools/graphics/vulkan/build-container.sh

destination="$task_root/bundles/experimental/fs/systems/linux-aarch64"
mkdir -p "$destination/usr/lib/aarch64-linux-gnu" "$destination/usr/share/vulkan/icd.d" "$destination/opt/sgfx-vulkan-tests"
install -m 755 "$output/target-sgfx/aarch64-unknown-linux-gnu/release/libvulkan_sgfx.so" "$destination/usr/lib/aarch64-linux-gnu/"
install -m 755 "$output/target-sws/aarch64-unknown-linux-gnu/release/libsws_client_c.so" "$destination/usr/lib/aarch64-linux-gnu/"
install -m 644 "$recipe/sgfx.json" "$destination/usr/share/vulkan/icd.d/"
for binary in vulkan-offscreen vulkan-offscreen.exe vulkan-present vulkan-present.exe vulkan-kernel-io; do
    install -m 755 "$output/$binary" "$destination/opt/sgfx-vulkan-tests/"
done
install -m 755 "$task_root/guest_tests/linux_vulkan/run.sh" "$destination/opt/sgfx-vulkan-tests/"
echo "Installed Linux ICD and probes in $destination"
