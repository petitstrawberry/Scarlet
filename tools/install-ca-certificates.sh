#!/bin/sh
set -eu

SCARLET_BUILD_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DESTINATION=$1
CA_PACKAGE=$(nix build --accept-flake-config --no-link --print-out-paths "$SCARLET_BUILD_ROOT#scarlet-ca-certificates")
CA_BUNDLE=$CA_PACKAGE/etc/ssl/certs/ca-bundle.crt

if [ ! -f "$CA_BUNDLE" ] || ! grep -q -- '-----BEGIN CERTIFICATE-----' "$CA_BUNDLE"; then
    echo "install-ca-certificates: invalid Nix CA bundle: $CA_BUNDLE" >&2
    exit 1
fi

mkdir -p "$(dirname -- "$DESTINATION")"
cp "$CA_BUNDLE" "$DESTINATION"
chmod 0644 "$DESTINATION"
