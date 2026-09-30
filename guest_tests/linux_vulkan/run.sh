#!/bin/bash
set -u
directory="$(cd -- "$(dirname -- "$0")" && pwd)"
export LC_ALL=C
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
case "${1:-linux}" in
linux)
    "$directory/vulkan-kernel-io" || exit $?
    "$directory/vulkan-offscreen" || exit $?
    "$directory/vulkan-present" || exit $?
    ;;
wine)
    export WINEPREFIX="${WINEPREFIX:-/var/tmp/wine-vulkan-tests}"
    export WINEDEBUG="${WINEDEBUG:--all}"
    for program in vulkan-offscreen.exe vulkan-present.exe; do
        /usr/local/bin/wine "$directory/$program"
        result=$?
        printf '%s: Wine launcher exit=%s\n' "$program" "$result"
        if [[ $result != 0 ]]; then exit "$result"; fi
    done
    ;;
*) echo "Usage: $0 [linux|wine]" >&2; exit 2 ;;
esac
