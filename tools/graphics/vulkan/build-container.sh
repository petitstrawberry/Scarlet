#!/bin/sh
set -eu
# The source mounts are read-only. Standalone SDK crates can generate their
# Cargo.lock inside the container, without changing the host checkout.
cp -a /scarlet/user/lib /tmp/scarlet-vulkan-sdk
sdk=/tmp/scarlet-vulkan-sdk/sws-client-c/Cargo.toml
export CARGO_TARGET_DIR=/out/target-sws
cargo build --release --target aarch64-unknown-linux-gnu --manifest-path "$sdk"
export LD_LIBRARY_PATH=/out/target-sws/aarch64-unknown-linux-gnu/release
export RUSTFLAGS="-Lnative=$LD_LIBRARY_PATH"
export CARGO_TARGET_DIR=/out/target-sgfx
cargo test --locked --release --target aarch64-unknown-linux-gnu \
    -p vulkan-sgfx --lib --no-default-features --features scarlet-wsi
cargo build --locked --release --target aarch64-unknown-linux-gnu \
    -p vulkan-sgfx --lib --no-default-features --features scarlet-wsi
mkdir -p /out/include
cp -a /usr/include/vulkan /usr/include/vk_video /out/include/
wayland-scanner client-header /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml /out/include/xdg-shell-client.h
wayland-scanner private-code /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml /out/xdg-shell-protocol.c
tests=/scarlet/guest_tests/linux_vulkan
cc -O2 -Wall -Wextra "$tests/offscreen.c" -ldl -o /out/vulkan-offscreen
cc -O2 -Wall -Wextra -I/out/include "$tests/present.c" /out/xdg-shell-protocol.c -lwayland-client -ldl -o /out/vulkan-present
cc -O2 -Wall -Wextra "$tests/kernel_io.c" -o /out/vulkan-kernel-io
x86_64-w64-mingw32-gcc -O2 -Wall -Wextra -I/out/include "$tests/offscreen.c" -static-libgcc -o /out/vulkan-offscreen.exe
x86_64-w64-mingw32-gcc -O2 -Wall -Wextra -Wno-cast-function-type -I/out/include "$tests/present.c" -static-libgcc -o /out/vulkan-present.exe
