#!/bin/sh
set -eu

shared=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
project=$(CDPATH= cd -- "${SCARLET_IOS_PROJECT:-$shared}" && pwd)
export SCARLET_IOS_PROJECT="$project"
export DEVELOPER_DIR=${SCARLET_IOS_DEVELOPER_DIR:-$(env -u DEVELOPER_DIR /usr/bin/xcode-select -p)}
unset SDKROOT
export IPHONEOS_DEPLOYMENT_TARGET=16.0

operation=$1
shift
if [ "$operation" = run ]; then
    for arg do
        case "$arg" in
            --) break ;;
            aarch64-apple-ios|--target=aarch64-apple-ios)
                echo "The iOS runner uses Simulator. Use image for device packaging." >&2
                exit 2
                ;;
        esac
    done
fi

# The shared bundles include both std and no_std packages. Use the rebuilt
# sysroot as the default target std for all of them.
SCARLET_IOS_GUEST_SYSROOT=$(python3 "$shared/tools/prepare_guest_sysroot.py")
export SCARLET_IOS_GUEST_SYSROOT
export RUSTC_WRAPPER="$shared/tools/guest_rustc.sh"

# Source-owned recipes inherit this cache too. Default to the BSP output path,
# but preserve caller overrides and resolve them before the SDK changes cwd.
# SDK-managed guest layers supply their own per-project target directories.
CARGO_TARGET_DIR=$(python3 "$shared/tools/image_config.py" "$project" target-dir)
export CARGO_TARGET_DIR

# Source-owned C/C++ builds use the SDK's native compiler entries. Apply the
# same register reservation as the Rust toolchain without changing the recipes.
export CFLAGS_aarch64_unknown_scarlet="${CFLAGS_aarch64_unknown_scarlet:+$CFLAGS_aarch64_unknown_scarlet }-ffixed-x18"
export CXXFLAGS_aarch64_unknown_scarlet="${CXXFLAGS_aarch64_unknown_scarlet:+$CXXFLAGS_aarch64_unknown_scarlet }-ffixed-x18"
if [ -n "${SCARLET_NATIVE_CC:-}" ]; then
    export SCARLET_IOS_NATIVE_CC="$SCARLET_NATIVE_CC"
    export SCARLET_NATIVE_CC="$shared/tools/guest_cc.sh"
fi
if [ -n "${SCARLET_NATIVE_CXX:-}" ]; then
    export SCARLET_IOS_NATIVE_CXX="$SCARLET_NATIVE_CXX"
    export SCARLET_NATIVE_CXX="$shared/tools/guest_cxx.sh"
fi

exec python3 "$shared/tools/sdk_command.py" "$project" "$operation" "$@"
