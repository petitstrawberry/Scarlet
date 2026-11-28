#!/bin/bash
set -euo pipefail

shared=$(cd "$(dirname "$0")/.." && pwd)
project=${SCARLET_PROJECT_DIR:-${SCARLET_IOS_PROJECT:-$shared}}
export SCARLET_IOS_PROJECT="$project"
case "$SCARLET_TARGET_TRIPLE" in
    aarch64-apple-ios-sim) sdk=iphonesimulator ;;
    aarch64-apple-ios) sdk=iphoneos ;;
    *) echo "Unsupported iOS BSP target: $SCARLET_TARGET_TRIPLE" >&2; exit 2 ;;
esac

path="$project/.scarlet/images/$sdk-$SCARLET_PROFILE.app-path"
"$shared/tools/build_app.sh" "$sdk" "$project/.scarlet/images/$sdk" > "$path.tmp"
mv "$path.tmp" "$path"
