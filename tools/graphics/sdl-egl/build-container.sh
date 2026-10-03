#!/bin/sh
set -eu
cd /out
task_output=/out
# Shared fork, fixed revision; no adjacent patch files are applied.
revision=fc96e4aff56cb40b738b80674fde36291d7a98aa
source="${task_output}/SDL2-2.30.0-$revision"
if [ ! -d "$source/.git" ]; then
    git init "$source"
    git -C "$source" remote add origin https://github.com/petitstrawberry/SDL.git
    git -C "$source" fetch --depth 1 origin "$revision"
    git -C "$source" checkout --detach FETCH_HEAD
fi
[ "$(git -C "$source" rev-parse HEAD)" = "$revision" ]
git -C "$source" diff --exit-code HEAD --
cmake -S "$source" -B build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/sgfx-sdl \
    -DSDL_SHARED=ON -DSDL_STATIC=OFF -DSDL_TEST=OFF -DSDL_TESTS=OFF \
    -DSDL_WAYLAND=ON -DSDL_WAYLAND_LIBDECOR=OFF
cmake --build build --parallel "${BUILD_JOBS:-4}"
DESTDIR=/out/runtime cmake --install build
sha256sum runtime/opt/sgfx-sdl/lib/libSDL2-2.0.so.0.3000.0 > runtime-sha256.txt
readelf -d runtime/opt/sgfx-sdl/lib/libSDL2-2.0.so.0.3000.0 > elf-dynamic.txt
