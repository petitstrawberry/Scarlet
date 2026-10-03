#!/bin/sh
set -eu
app=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$app"
# All loader selection is scoped to this process. Explicit sdl-opengl fails
# instead of silently selecting OpenTTD's software video driver.
export LD_LIBRARY_PATH="${SGFX_SDL_RUNTIME:-/opt/sgfx-sdl}/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}"
exec /opt/sgfx-vulkan-tests/zink-window-run ./openttd \
    -X -x -c ./openttd-zink.cfg -I OpenGFX -v sdl-opengl \
    -b 40bpp-anim -s null -m null -r 628x300 -d driver=2 -g -G 12345 "$@"
