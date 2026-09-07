#!/bin/bash

# Select an accelerator for a full project without starting a VM.
# Arguments: QEMU system emulator executable, guest architecture.
# SCARLET_QEMU_ACCEL always takes precedence over host detection.
scarlet_qemu_accel() {
    if [ -n "${SCARLET_QEMU_ACCEL:-}" ]; then
        printf '%s\n' "$SCARLET_QEMU_ACCEL"
        return 0
    fi

    local host_arch candidate available_accels
    host_arch="$(uname -m)"
    case "$host_arch" in
        arm64) host_arch=aarch64 ;;
    esac
    if [ "$host_arch" != "$2" ]; then
        printf '%s\n' tcg
        return 0
    fi

    case "$(uname -s)" in
        Darwin)
            if [ "$host_arch" != "aarch64" ] || [ "$(sysctl -n kern.hv_support 2>/dev/null)" != "1" ]; then
                printf '%s\n' tcg
                return 0
            fi
            candidate=hvf
            ;;
        Linux)
            if [ ! -c /dev/kvm ] || [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
                printf '%s\n' tcg
                return 0
            fi
            candidate=kvm
            ;;
        *)
            printf '%s\n' tcg
            return 0
            ;;
    esac

    if ! available_accels=$("$1" -accel help); then
        printf 'Error: could not query accelerators from %s\n' "$1" >&2
        return 1
    fi
    if printf '%s\n' "$available_accels" | grep -Fxq "$candidate"; then
        printf '%s\n' "$candidate"
    else
        printf '%s\n' tcg
    fi
}
