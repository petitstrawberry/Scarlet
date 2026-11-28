#!/bin/sh
set -eu

project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export SCARLET_IOS_PROJECT="$project"
export BUNDLE_ID=${BUNDLE_ID:-org.scarletos.ios.desktop}
export SCARLET_IOS_DEFAULT_DISPLAY=1
export SCARLET_IOS_DISPLAY_NAME="Scarlet Desktop"
exec "$project/../aarch64-user-ios-minimum/tools/package_image.sh" "$@"
