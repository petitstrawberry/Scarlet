#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -n "${SCARLET_SGFX_SOURCE:-}" ]; then
    exec python3 "$script_dir/../sgfx-native-build.py" --project "$PWD" --install-dir "$1" --sgfx-source "$SCARLET_SGFX_SOURCE"
fi
exec python3 "$script_dir/../sgfx-native-build.py" --project "$PWD" --install-dir "$1"
