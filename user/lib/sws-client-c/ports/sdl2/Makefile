# Build the fullscreen SWS video driver against upstream SDL2 on AArch64 Linux.
# Example: make SWS_CLIENT_LIBRARY=/path/to/libsws_client_c.so
SDL_VERSION := 2.32.10
SDL_SHA256 := 5f5993c530f084535c65a6879e9b26ad441169b3e25d789d83287040a9ca5165
WORK_DIR ?= $(CURDIR)/build
SDL_SOURCE := $(WORK_DIR)/SDL2-$(SDL_VERSION)
SDL_BUILD := $(WORK_DIR)/release
SWS_CLIENT_LIBRARY ?= $(abspath ../../target/release/libsws_client_c.so)
SWS_CLIENT_INCLUDE_DIR := $(abspath ../../include)
JOBS ?= 4
PREFIX ?= /usr

.PHONY: all configure install
all: configure
	cmake --build "$(SDL_BUILD)" --parallel $(JOBS)

$(WORK_DIR)/SDL2.tar.gz:
	mkdir -p "$(WORK_DIR)"
	curl --fail --location --output "$@" "https://libsdl.org/release/SDL2-$(SDL_VERSION).tar.gz"

$(SDL_SOURCE)/.sws-patched: $(WORK_DIR)/SDL2.tar.gz SDL2-$(SDL_VERSION)-sws.patch
	printf '%s  %s\n' '$(SDL_SHA256)' '$(WORK_DIR)/SDL2.tar.gz' | sha256sum -c -
	tar -xzf "$<" -C "$(WORK_DIR)"
	patch -d "$(SDL_SOURCE)" -p1 < "SDL2-$(SDL_VERSION)-sws.patch"
	mkdir -p "$(SDL_SOURCE)/src/video/sws"
	touch "$@"

configure: $(SDL_SOURCE)/.sws-patched SDL_swsvideo.c
	test -f "$(SWS_CLIENT_LIBRARY)"
	cp SDL_swsvideo.c "$(SDL_SOURCE)/src/video/sws/"
	cmake -S "$(SDL_SOURCE)" -B "$(SDL_BUILD)" -G Ninja \
	  -DCMAKE_BUILD_TYPE=Release -DCMAKE_SKIP_RPATH=ON \
	  -DCMAKE_INSTALL_PREFIX="$(PREFIX)" -DSDL_SWS=ON \
	  -DSWS_CLIENT_INCLUDE_DIR="$(SWS_CLIENT_INCLUDE_DIR)" \
	  -DSWS_CLIENT_LIBRARY="$(SWS_CLIENT_LIBRARY)" \
	  -DSDL_SHARED=ON -DSDL_STATIC=OFF -DSDL_TEST=OFF \
	  -DSDL_X11=OFF -DSDL_WAYLAND=OFF -DSDL_KMSDRM=OFF \
	  -DSDL_OPENGL=OFF -DSDL_OPENGLES=OFF -DSDL_PIPEWIRE=OFF \
	  -DSDL_PULSEAUDIO=OFF -DSDL_ALSA=OFF -DSDL_JACK=OFF \
	  -DSDL_HIDAPI=OFF -DSDL_LIBUDEV=OFF

install: all
	DESTDIR="$(DESTDIR)" cmake --install "$(SDL_BUILD)"
