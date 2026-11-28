#!/bin/sh
set -eu

project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export SCARLET_IOS_PROJECT="$project"
exec "$project/../aarch64-user-ios-minimum/tools/run_ios.sh" "$@"
