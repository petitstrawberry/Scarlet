#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
    set -- bash
fi

if [ ! -f flake.nix ]; then
    echo "Scarlet flake.nix not found in $(pwd)." >&2
    echo "Mount the repository at /workspaces/Scarlet or set the container working directory to the checkout." >&2
    exec "$@"
fi

if [ -n "${SCARLET_CACHIX_CACHE:-}" ] && [ -n "${SCARLET_CACHIX_PUBLIC_KEY:-}" ]; then
    cache_url="https://${SCARLET_CACHIX_CACHE}.cachix.org"
    nix_conf=/etc/nix/nix.conf

    if ! grep -q "${cache_url}" "${nix_conf}"; then
        {
            echo "extra-substituters = ${cache_url}"
            echo "extra-trusted-public-keys = ${SCARLET_CACHIX_PUBLIC_KEY}"
        } >> "${nix_conf}"
    fi
fi

if [ -d /opt/scarlet-rust-build ]; then
    export SCARLET_RUST_BUILD_ROOT=/opt/scarlet-rust-build
fi

exec nix develop .#default --command "$@"
