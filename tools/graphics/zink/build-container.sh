#!/bin/sh
set -eu
version=25.0.7
task_zink_output="${ZINK_OUTPUT:-/out}"
task_output="$task_zink_output"
# Shared fork, fixed revision; no adjacent patch files are applied.
revision=a77e1dae360bd306ede96904fa9f42eb638bf406
source="${task_output}/mesa-25.0.7-$revision"
if [ ! -d "$source/.git" ]; then
    git init "$source"
    git -C "$source" remote add origin https://github.com/petitstrawberry/mesa.git
    git -C "$source" fetch --depth 1 origin "$revision"
    git -C "$source" checkout --detach FETCH_HEAD
fi
[ "$(git -C "$source" rev-parse HEAD)" = "$revision" ]
git -C "$source" diff --exit-code HEAD --
set --
if [ -f "$task_zink_output/build/meson-private/coredata.dat" ]; then set -- --reconfigure; fi
meson setup "$@" "$task_zink_output/build" "$source" \
    --prefix=/opt/sgfx-zink --libdir=lib/aarch64-linux-gnu \
    -Dbuildtype=release -Db_ndebug=true \
    -Dgallium-drivers=zink,softpipe -Dvulkan-drivers= -Dplatforms=wayland \
    -Dglx=disabled -Degl=enabled -Dgbm=enabled -Dglvnd=enabled \
    -Dllvm=disabled -Dvideo-codecs= -Dgallium-vdpau=disabled -Dgallium-va=disabled
ninja -C "$task_zink_output/build" -j "${BUILD_JOBS:-8}"
DESTDIR="$task_zink_output/runtime" ninja -C "$task_zink_output/build" install
lib="$task_zink_output/runtime/opt/sgfx-zink/lib/aarch64-linux-gnu"
# The standalone EGL loader keeps this benchmark independent of Wine/GLX.
cp -L /usr/lib/aarch64-linux-gnu/libEGL.so.1 "$lib/"
cp -L /usr/lib/aarch64-linux-gnu/libGLdispatch.so.0 "$lib/"
for name in libGL.so.1 libGLX.so.0 libOpenGL.so.0; do
    cp -L "/usr/lib/aarch64-linux-gnu/$name" "$lib/"
done
strip --strip-debug "$lib"/*.so*
mkdir -p "$task_zink_output/runtime/opt/sgfx-vulkan-tests"
cc -O2 -Wall -Wextra -Werror /scarlet/guest_tests/linux_zink/egl.c \
    -lEGL -o "$task_zink_output/runtime/opt/sgfx-vulkan-tests/zink-egl"
install -m 755 /scarlet/guest_tests/linux_zink/run-egl.sh "$task_zink_output/runtime/opt/sgfx-vulkan-tests/zink-run"
cc -O2 -Wall -Wextra -Werror /scarlet/guest_tests/linux_zink/window.c \
    $(pkg-config --cflags --libs sdl2) -lGL \
    -o "$task_zink_output/runtime/opt/sgfx-vulkan-tests/zink-window"
install -m 755 /scarlet/guest_tests/linux_zink/run-window.sh "$task_zink_output/runtime/opt/sgfx-vulkan-tests/zink-window-run"
cc -O2 -Wall -Wextra -Werror /scarlet/guest_tests/linux_zink/wayland-shm-resize.c \
    -lwayland-client -o "$task_zink_output/runtime/opt/sgfx-vulkan-tests/wayland-shm-resize"

protocols=$(pkg-config --variable=pkgdatadir wayland-protocols)
generated="$task_zink_output/decoration-protocols"
mkdir -p "$generated"
wayland-scanner client-header "$protocols/stable/xdg-shell/xdg-shell.xml" "$generated/xdg-shell-client-protocol.h"
wayland-scanner private-code "$protocols/stable/xdg-shell/xdg-shell.xml" "$generated/xdg-shell-protocol.c"
wayland-scanner client-header "$protocols/unstable/xdg-decoration/xdg-decoration-unstable-v1.xml" "$generated/xdg-decoration-unstable-v1-client-protocol.h"
wayland-scanner private-code "$protocols/unstable/xdg-decoration/xdg-decoration-unstable-v1.xml" "$generated/xdg-decoration-unstable-v1-protocol.c"
cc -O2 -Wall -Wextra -Werror -I"$generated" \
    /scarlet/guest_tests/linux_zink/wayland-decoration.c \
    "$generated/xdg-shell-protocol.c" "$generated/xdg-decoration-unstable-v1-protocol.c" \
    -lwayland-client -o "$task_zink_output/runtime/opt/sgfx-vulkan-tests/wayland-decoration"
