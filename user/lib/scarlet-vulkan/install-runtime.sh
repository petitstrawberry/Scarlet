#!/bin/sh
set -eu
# Runtime resources only. Cargo layers have already built and staged the libraries.
package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -f "$1/lib/libvulkan.so.1" ]; then
    cp -R "$package_dir/files/." "$1/"
    ln -sf libvulkan.so.1 "$1/lib/libvulkan.so"
    mkdir -p "$1/share/licenses/LLVM"
    cp "$SCARLET_LLVM_LIBC_SOURCE_DIR/../LICENSE.TXT" "$1/share/licenses/LLVM/LICENSE.TXT"
fi
