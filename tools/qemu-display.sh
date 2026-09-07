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
            printf '%s\n' 'cocoa,gl=on,retina=on,full-grab=on'
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
                        if [ "$backend" = "gtk" ]; then
                            printf '%s,gl=on,grab-on-hover=on\n' "$backend"
                        else
                            printf '%s,gl=on\n' "$backend"
                        fi
                        return 0
                    fi
                done
            fi
            ;;
    esac

    # Preserve remote access when there is no local GUI session or backend.
    printf '%s\n' 'vnc=:0'
}

# Match the full project's GPU to the selected display's GL mode.
# Arguments: selected QEMU display (including options).
# SCARLET_QEMU_GPU always takes precedence over display selection.
scarlet_qemu_gpu() {
    if [ -n "${SCARLET_QEMU_GPU:-}" ]; then
        printf '%s\n' "$SCARLET_QEMU_GPU"
        return 0
    fi

    case ",$1," in
        *,gl=on,*|*,gl=core,*|*,gl=es,*)
            printf '%s\n' virtio-gpu-gl-pci
            ;;
        *)
            printf '%s\n' virtio-gpu-pci
            ;;
    esac
}
