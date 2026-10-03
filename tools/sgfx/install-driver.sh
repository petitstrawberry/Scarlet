#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$script_dir/../sgfx-native-build.py" --project "$PWD" --install-dir "$1"
