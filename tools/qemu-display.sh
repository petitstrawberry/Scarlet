#!/bin/bash

# Select a full project's display without changing the serial console.
# Arguments: QEMU system emulator executable.
# SCARLET_QEMU_DISPLAY always takes precedence over host detection.
scarlet_qemu_display() {
    if [ -n "${SCARLET_QEMU_DISPLAY:-}" ]; then
        printf '%s\n' "$SCARLET_QEMU_DISPLAY"
        return 0
    fi

    case "$(uname -s)" in
        Darwin)
            printf '%s\n' cocoa
            return 0
            ;;
        Linux)
            if [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
                local available_displays backend
                if ! available_displays=$("$1" -display help); then
                    printf 'Error: could not query display backends from %s\n' "$1" >&2
                    return 1
                fi
                for backend in gtk sdl; do
                    if printf '%s\n' "$available_displays" | grep -Fxq "$backend"; then
                        printf '%s\n' "$backend"
                        return 0
                    fi
                done
            fi
            ;;
    esac

    # Preserve remote access when there is no local GUI session or backend.
    printf '%s\n' 'vnc=:0'
}
