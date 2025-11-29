FROM ubuntu:25.04

ENV PATH=/root/.cargo/bin:/opt/bin:/opt/buildroot/output/host/bin:$PATH
ENV MAKEFLAGS=-j$(($(nproc)-2))
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

ENV DEBIAN_FRONTEND noninteractive

# Install dependencies and tools
RUN apt update && \
	apt install -y build-essential autoconf automake autotools-dev curl bc git device-tree-compiler vim python3 python3-venv gdb-multiarch gcc-riscv64-linux-gnu cpio libncurses5-dev libncursesw5-dev \
    mtools dosfstools sleuthkit libslirp-dev

# # # Install QEMU
# RUN apt install -y qemu-system-riscv64

RUN apt update && \
    apt install -y pkg-config libglib2.0-dev libmount-dev python3 python3-venv python3-pip python3-dev git libssl-dev libffi-dev build-essential automake libfreetype6-dev libtheora-dev libtool libvorbis-dev pkg-config texinfo zlib1g-dev unzip cmake yasm libx264-dev libmp3lame-dev libopus-dev libvorbis-dev libxcb1-dev libxcb-shm0-dev libxcb-xfixes0-dev pkg-config texinfo wget zlib1g-dev ninja-build libpixman-1-dev
RUN cd /opt && \
    wget https://download.qemu.org/qemu-10.1.2.tar.xz && \
	tar xvJf qemu-10.1.2.tar.xz && \
	rm qemu-10.1.2.tar.xz && \
	cd qemu-10.1.2 && \
    ./configure --target-list=riscv32-softmmu,riscv64-softmmu --prefix=/opt --enable-slirp --python=/usr/bin/python3 && \
	make -j 8 && \
	make install

# Install Rust and RISC-V target
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y && \
    rustup default nightly-2025-04-28 && \
    rustup install nightly-2025-04-28 && \
    rustup component add rust-src --toolchain nightly-2025-04-28 && \
    rustup target add riscv64gc-unknown-none-elf

# Install cargo tools
RUN cargo install cargo-make

# Build xv6 and the user programs
RUN git clone https://github.com/mit-pdos/xv6-riscv.git /opt/xv6-riscv && \
    cd /opt/xv6-riscv && \
    git checkout 2a39c5af63906b3dbd0db58b9f6846ad70f4315d && \
    make fs.img

# Build octox and the user programs
RUN git clone https://github.com/o8vm/octox.git /opt/octox && \
    cd /opt/octox && \
    git checkout fd1dc60d89fcd1e787bccaf1af85c3f48552c33d && \
    cargo build --target riscv64gc-unknown-none-elf

# Install dependencies for Buildroot
RUN apt update && \
    apt install -y libncurses5-dev wget unzip rsync

# Download and set up Buildroot
RUN cd /opt && \
    wget https://buildroot.org/downloads/buildroot-2025.02.6.tar.gz && \
    tar -xvf buildroot-2025.02.6.tar.gz && \
    rm buildroot-2025.02.6.tar.gz && \
    mv buildroot-2025.02.6 buildroot

# Copy configuration files for Buildroot
COPY docker/.config /opt/buildroot/.config

# # Create patches directory and copy LTP musl compatibility patch
# RUN mkdir -p /opt/buildroot/package/ltp-testsuite/patches
# COPY docker/0001-exclude-listmount-statmount-for-musl.patch /opt/buildroot/package/ltp-testsuite/patches/

# Buildroot compilation now handled by tools/linux/build_buildroot.sh
# User program builds (green, fbdoom) handled by tools/linux/build_user_programs.sh

WORKDIR /workspaces/Scarlet