#!/bin/sh
set -eu
# The source mounts are read-only. Standalone SDK crates can generate their
# Cargo.lock inside the container, without changing the host checkout.
task_sdk_directory=$(mktemp -d /tmp/scarlet-vulkan-sdk.XXXXXX)
trap 'rm -rf "$task_sdk_directory"' EXIT HUP INT TERM
tar --exclude=target --exclude=.git -cf "$task_sdk_directory/source.tar" -C /scarlet/user/lib .
tar -xf "$task_sdk_directory/source.tar" -C "$task_sdk_directory"
rm "$task_sdk_directory/source.tar"
sdk="$task_sdk_directory/sws-client-c/Cargo.toml"
export CARGO_TARGET_DIR=/out/target-sws
cargo build --release --target aarch64-unknown-linux-gnu --manifest-path "$sdk"
export LD_LIBRARY_PATH=/out/target-sws/aarch64-unknown-linux-gnu/release
export RUSTFLAGS="-Lnative=$LD_LIBRARY_PATH"
set -- --output /out --sws-library "$LD_LIBRARY_PATH/libsws_client_c.so" --test
if [ -n "${SGFX_MAXWELL_SOURCE:-}" ]; then
    set -- "$@" --maxwell-source "$SGFX_MAXWELL_SOURCE"
fi
python3 /sgfx/scripts/build-linux-icd.py "$@"
# Probe sources are optional in distribution revisions that do not ship them.
if [ ! -f /scarlet/guest_tests/linux_vulkan/offscreen.c ] || [ ! -f /scarlet/guest_tests/linux_zink/requirements.c ]; then
    echo "ICD and plugins built; this distribution has no Linux Vulkan/Zink probe sources."
    exit 0
fi
mkdir -p /out/include
cp -a /usr/include/vulkan /usr/include/vk_video /out/include/
wayland-scanner client-header /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml /out/include/xdg-shell-client.h
wayland-scanner private-code /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml /out/xdg-shell-protocol.c
tests=/scarlet/guest_tests/linux_vulkan
cc -O2 -Wall -Wextra "$tests/offscreen.c" -ldl -o /out/vulkan-offscreen
cc -O2 -Wall -Wextra -I/out/include "$tests/present.c" /out/xdg-shell-protocol.c -lwayland-client -ldl -o /out/vulkan-present
cc -O2 -Wall -Wextra "$tests/kernel_io.c" -o /out/vulkan-kernel-io
cc -O2 -Wall -Wextra -Werror /scarlet/guest_tests/linux_zink/requirements.c -ldl -o /out/zink-requirements
cc -O2 -Wall -Wextra -Werror /scarlet/guest_tests/linux_zink/timeline.c -ldl -o /out/zink-timeline
glslangValidator -V --vn viewport_vert /scarlet/guest_tests/linux_zink/viewport.vert -o /out/include/viewport-vert.h
glslangValidator -V --vn viewport_frag /scarlet/guest_tests/linux_zink/viewport.frag -o /out/include/viewport-frag.h
cc -O2 -Wall -Wextra -Werror -I/out/include /scarlet/guest_tests/linux_zink/viewport.c -ldl -o /out/zink-viewport
x86_64-w64-mingw32-gcc -O2 -Wall -Wextra -I/out/include "$tests/offscreen.c" -static-libgcc -o /out/vulkan-offscreen.exe
x86_64-w64-mingw32-gcc -O2 -Wall -Wextra -Wno-cast-function-type -I/out/include "$tests/present.c" -static-libgcc -o /out/vulkan-present.exe
