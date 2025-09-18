FROM ubuntu:24.04

ENV PATH=/root/.cargo/bin:$PATH
ENV MAKEFLAGS=-j$(($(nproc)-2))
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

ENV DEBIAN_FRONTEND noninteractive

# Install dependencies and tools
RUN apt update && \
	apt install -y build-essential autoconf automake autotools-dev curl bc git device-tree-compiler vim python3 gdb-multiarch gcc-riscv64-linux-gnu cpio libncurses5-dev libncursesw5-dev \
  mtools dosfstools sleuthkit

# Install QEMU
RUN apt install -y qemu-system-riscv64

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

# Download and configure busybox
RUN mkdir -p /opt && cd /opt && \
    git clone https://git.busybox.net/busybox && \
    cd busybox && \
    make defconfig && \
    # Disable TC (Traffic Control) features to avoid CBQ compilation errors
    sed -i 's/CONFIG_TC=y/# CONFIG_TC is not set/' .config && \
    sed -i 's/CONFIG_FEATURE_TC_INGRESS=y/# CONFIG_FEATURE_TC_INGRESS is not set/' .config && \
    sed -i 's/# CONFIG_STATIC is not set/CONFIG_STATIC=y/' .config && \
    make CROSS_COMPILE=riscv64-linux-gnu- all && \
    make CROSS_COMPILE=riscv64-linux-gnu- install

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

# Get source code 
RUN cd /opt/buildroot && \
    make source

# Patch 

WORKDIR /workspaces/Scarlet