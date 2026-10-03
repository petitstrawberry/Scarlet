#!/usr/bin/env bash
set -euo pipefail
scarlet_root="$(cd "$(dirname "$0")/../../.." && pwd)"
output="${SCARLET_OPENTTD_OUTPUT:-$scarlet_root/artifacts/linux-openttd-build}"
recipe="$scarlet_root/tools/graphics/openttd"
image="${SCARLET_OPENTTD_IMAGE:-scarlet-linux-openttd-build}"
mkdir -p "$output"
docker build --platform linux/arm64 -t scarlet-linux-vulkan-build "$scarlet_root/tools/graphics/vulkan"
docker build --platform linux/arm64 -t "$image" "$recipe"
docker run --rm --platform linux/arm64 --read-only --tmpfs /tmp \
    --mount "type=bind,source=$recipe,target=/recipe,readonly" \
    --mount "type=bind,source=$output,target=/out" \
    "$image" sh /recipe/build-container.sh
printf 'OpenTTD runtime: %s/runtime/opt/openttd-zink\n' "$output"
