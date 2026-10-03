#!/bin/sh
set -eu
cd /out
task_output=/out
# Shared fork, fixed revision; no adjacent patch files are applied.
revision=75b543bc25192609092e8024928f37c4af95f712
source="${task_output}/OpenTTD-15.3-$revision"
if [ ! -d "$source/.git" ]; then
    git init "$source"
    git -C "$source" remote add origin https://github.com/petitstrawberry/OpenTTD.git
    git -C "$source" fetch --depth 1 origin "$revision"
    git -C "$source" checkout --detach FETCH_HEAD
fi
[ "$(git -C "$source" rev-parse HEAD)" = "$revision" ]
git -C "$source" diff --exit-code HEAD --
cmake -S "$source" -B build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/openttd-zink \
    -DOPTION_INSTALL_FHS=OFF -DOPTION_DEDICATED=OFF \
    -DCMAKE_DISABLE_FIND_PACKAGE_Allegro=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_CURL=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_Freetype=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_Fontconfig=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_Harfbuzz=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_ICU=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_Fluidsynth=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_OpusFile=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_LZO=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_unofficial-breakpad=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_Grfcodec=ON \
    -DCMAKE_DISABLE_FIND_PACKAGE_Doxygen=ON
cmake --build build --target openttd --parallel "${BUILD_JOBS:-4}"
DESTDIR=/out/runtime cmake --install build
archive=opengfx-8.0-all.zip
if [ ! -f "$archive" ]; then
    curl -fL --retry 3 https://cdn.openttd.org/opengfx-releases/8.0/opengfx-8.0-all.zip -o "$archive"
fi
printf '%s  %s\n' 43a0c1dabf39cb865394f3a6cc36d4da5c10ecfaaf55652043104806810903be "$archive" | sha256sum -c -
app=runtime/opt/openttd-zink
mkdir -p "$app/baseset"
unzip -p "$archive" opengfx-8.0.tar > "$app/baseset/opengfx-8.0.tar"
install -m 644 "$source/COPYING.md" "$app/COPYING.md"
install -m 644 /recipe/openttd-zink.cfg "$app/openttd-zink.cfg"
install -m 755 /recipe/run-zink.sh "$app/run-zink.sh"
sha256sum "$app/openttd" "$app/baseset/opengfx-8.0.tar" > runtime-sha256.txt
readelf -d "$app/openttd" > elf-dynamic.txt
