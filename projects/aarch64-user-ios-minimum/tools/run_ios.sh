#!/bin/sh
set -eu

project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
profile=debug
[ "${SCARLET_RELEASE:-0}" != 1 ] || profile=release
app=$(cat "$project/.scarlet/images/iphonesimulator-$profile.app-path")
identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Info.plist")
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl install booted "$app"
exec env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl launch \
    --terminate-running-process --console booted "$identifier" "$@"
