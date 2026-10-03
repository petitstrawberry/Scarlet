FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential cmake ninja-build pkg-config \
    libsdl2-dev libgl-dev libwayland-dev wayland-protocols \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /out
