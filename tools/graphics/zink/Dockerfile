FROM scarlet-linux-vulkan-build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    meson ninja-build bison flex python3-mako python3-yaml python3-packaging \
    libegl1-mesa-dev libgl-dev libglvnd-dev libdrm-dev libexpat1-dev libzstd-dev \
    libelf-dev libudev-dev libsdl2-dev libwayland-egl-backend-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /out
