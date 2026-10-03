FROM scarlet-linux-vulkan-build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    cmake ninja-build pkg-config libsdl2-dev libgl-dev libopengl-dev \
    zlib1g-dev liblzma-dev libpng-dev unzip \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /out
