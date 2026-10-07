#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/../../.." && pwd)"
if [[ $# -lt 1 ]]; then
    echo "Usage: $0 <sgfx-source-directory> [--maxwell-source <switch>] [--output <directory>] [--stage-only]" >&2
    exit 2
fi
sgfx_source="$(cd "$1" && pwd)"
output="$task_root/artifacts/linux-vulkan-build"
shift
maxwell_source=""
stage_only=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --maxwell-source) maxwell_source="$(cd "$2" && pwd)"; shift 2 ;;
        --output) output="$2"; shift 2 ;;
        --stage-only) stage_only=true; shift ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done
recipe="$task_root/tools/graphics/vulkan"
[[ -f "$sgfx_source/scripts/build-linux-icd.py" ]] || { echo "SGFX needs Linux dynamic ICD support" >&2; exit 2; }
[[ -f "$sgfx_source/crates/vulkan-sgfx/Cargo.toml" ]] || exit 2
cmp "$recipe/scarlet-sgfx.xml" "$sgfx_source/crates/vulkan-sgfx/protocol/scarlet-sgfx.xml"
mkdir -p "$output"
output="$(cd "$output" && pwd)"
extra_mount=()
if [[ -n "$maxwell_source" ]]; then
    [[ -f "$maxwell_source/userspace/sgfx-backend-scarlet-maxwell-plugin/Cargo.toml" ]] || exit 2
    extra_mount=(-v "$maxwell_source:/switch:ro" -e SGFX_MAXWELL_SOURCE=/switch)
fi
docker build --platform linux/arm64 -t scarlet-linux-vulkan-build -f "$recipe/Dockerfile" "$recipe"
docker run --rm --platform linux/arm64 \
    -v "$task_root:/scarlet:ro" -v "$sgfx_source:/sgfx:ro" -v "$output:/out" \
    "${extra_mount[@]}" scarlet-linux-vulkan-build sh /scarlet/tools/graphics/vulkan/build-container.sh

if $stage_only; then
    echo "Linux ICD and drivers staged in $output/rootfs; report: $output/build.json"
    exit 0
fi
destination="$task_root/bundles/experimental/fs/systems/linux-aarch64"
mkdir -p "$destination"
cp -a "$output/rootfs/." "$destination/"
if [[ -f "$task_root/guest_tests/linux_vulkan/offscreen.c" && -f "$task_root/guest_tests/linux_zink/requirements.c" ]]; then
    mkdir -p "$destination/opt/sgfx-vulkan-tests"
    for binary in vulkan-offscreen vulkan-offscreen.exe vulkan-present vulkan-present.exe vulkan-kernel-io zink-requirements zink-timeline zink-viewport; do
        install -m 755 "$output/$binary" "$destination/opt/sgfx-vulkan-tests/"
    done
    install -m 755 "$task_root/guest_tests/linux_vulkan/run.sh" "$destination/opt/sgfx-vulkan-tests/"
fi
echo "Installed Linux ICD, dynamic drivers and available probes in $destination"
