FROM ubuntu:25.04

ENV PATH=/opt/scarlet-rust-toolchain/bin:/root/.cargo/bin:/opt/bin:/opt/buildroot/output/host/bin:$PATH
ENV MAKEFLAGS=-j$(($(nproc)-2))
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
ENV SCARLET_RUST_TARGET_TRIPLES="riscv64gc-unknown-scarlet aarch64-unknown-scarlet"
ENV CC_riscv64gc_unknown_scarlet_elf=riscv64-linux-gnu-gcc
ENV CFLAGS_riscv64gc_unknown_scarlet_elf="-march=rv64gc -mabi=lp64d -DRING_CORE_NOSTDLIBINC -fno-stack-protector"
ENV AR_riscv64gc_unknown_scarlet_elf=riscv64-linux-gnu-ar

ENV DEBIAN_FRONTEND=noninteractive

# Install dependencies and tools
RUN apt update && \
	apt install -y build-essential clang llvm autoconf automake autotools-dev curl bc git device-tree-compiler vim python3 python3-venv gdb-multiarch gcc-riscv64-linux-gnu gcc-aarch64-linux-gnu cpio libncurses5-dev libncursesw5-dev \
    mtools dosfstools sleuthkit libslirp-dev qemu-efi-riscv64 qemu-efi-aarch64

# # # Install QEMU
# RUN apt install -y qemu-system-riscv64

RUN apt update && \
    apt install -y pkg-config libglib2.0-dev libmount-dev python3 python3-venv python3-pip python3-dev git libssl-dev libffi-dev build-essential automake libfreetype6-dev libtheora-dev libtool libvorbis-dev pkg-config texinfo zlib1g-dev unzip cmake yasm libx264-dev libmp3lame-dev libopus-dev libvorbis-dev libxcb1-dev libxcb-shm0-dev libxcb-xfixes0-dev pkg-config texinfo wget zlib1g-dev ninja-build libpixman-1-dev libcapstone-dev
RUN cd /opt && \
    wget https://download.qemu.org/qemu-10.1.2.tar.xz && \
	tar xvJf qemu-10.1.2.tar.xz && \
	rm qemu-10.1.2.tar.xz && \
	cd qemu-10.1.2 && \
    ./configure --target-list=riscv32-softmmu,riscv64-softmmu,aarch64-softmmu --prefix=/opt --enable-slirp --python=/usr/bin/python3 --enable-debug --enable-capstone && \
	make -j 8 && \
	make install

# Build U-Boot for QEMU AArch64
# U-Boot provides proper boot protocol support (DTB in x0, etc.)
# Configure auto-boot with QEMU fw_cfg to load kernel passed via -kernel option
RUN apt update && \
    apt install -y bison flex libssl-dev python3-setuptools python3-pyelftools libgnutls28-dev && \
    cd /opt && \
    wget https://ftp.denx.de/pub/u-boot/u-boot-2025.01.tar.bz2 && \
    tar xjf u-boot-2025.01.tar.bz2 && \
    rm u-boot-2025.01.tar.bz2 && \
    cd u-boot-2025.01 && \
    make CROSS_COMPILE=aarch64-linux-gnu- qemu_arm64_defconfig && \
    sed -i 's/CONFIG_BOOTCOMMAND=.*/CONFIG_BOOTCOMMAND="qfw load 0x40200000 0x44000000; booti 0x40200000 0x44000000:${filesize} ${fdtcontroladdr}"/' .config && \
    sed -i 's/CONFIG_PREBOOT=.*/CONFIG_PREBOOT=""/' .config && \
    make CROSS_COMPILE=aarch64-linux-gnu- -j$(nproc) && \
    cp u-boot.bin /opt/u-boot-aarch64.bin

# Install Rust and architecture targets
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y && \
    rustup default nightly-2025-12-31 && \
    rustup install nightly-2025-12-31 && \
    rustup component add rust-src rustfmt clippy llvm-tools-preview --toolchain nightly-2025-12-31 && \
    rustup target add riscv64gc-unknown-none-elf && \
    rustup target add aarch64-unknown-none

# Install cargo tools
RUN cargo install cargo-make

# Build the Scarlet Rust fork as the container toolchain.
# The rustup nightly above is only a bootstrap compiler for this image build.
RUN mkdir -p /opt/scarlet-rust-src && \
    cd /opt/scarlet-rust-src && \
    case "$(uname -m)" in \
        x86_64) rust_host_triple="x86_64-unknown-linux-gnu" ;; \
        aarch64|arm64) rust_host_triple="aarch64-unknown-linux-gnu" ;; \
        *) echo "Unsupported Docker host architecture: $(uname -m)" >&2; exit 1 ;; \
    esac && \
    rustup_toolchain="/root/.rustup/toolchains/nightly-2025-12-31-${rust_host_triple}" && \
    git init && \
    git remote add origin https://github.com/petitstrawberry/rust.git && \
    git fetch --depth 1 origin b9573d6cd0731d24486f77ddf24d502e2e6bef02 && \
    git checkout FETCH_HEAD && \
    git submodule update --init --recursive --depth 1 --jobs "$(nproc)" && \
    printf '%s\n' \
      'change-id = "ignore"' \
      'profile = "compiler"' \
      '' \
      '[build]' \
      "build = \"${rust_host_triple}\"" \
      "host = [\"${rust_host_triple}\"]" \
      'target = ["riscv64gc-unknown-scarlet", "aarch64-unknown-scarlet"]' \
      "cargo = \"${rustup_toolchain}/bin/cargo\"" \
      "rustc = \"${rustup_toolchain}/bin/rustc\"" \
      "rustfmt = \"${rustup_toolchain}/bin/rustfmt\"" \
      'docs = false' \
      'submodules = false' \
      '' \
      '[llvm]' \
      'download-ci-llvm = false' \
      'targets = "AArch64;RISCV;X86"' \
      '' \
      '[rust]' \
      'download-rustc = false' \
      > bootstrap.toml && \
    ./x build --config bootstrap.toml compiler/rustc library && \
    mkdir -p /opt/scarlet-rust-toolchain && \
    cp -a "build/${rust_host_triple}/stage1/." /opt/scarlet-rust-toolchain/ && \
    for tool_path in "${rustup_toolchain}"/bin/*; do \
        tool="$(basename "$tool_path")"; \
        if [ "$tool" != rustc ]; then \
            ln -sfn "$tool_path" "/opt/scarlet-rust-toolchain/bin/$tool"; \
        fi; \
    done && \
    mkdir -p "/opt/scarlet-rust-toolchain/lib/rustlib/${rust_host_triple}/bin" && \
    for tool_path in "${rustup_toolchain}/lib/rustlib/${rust_host_triple}/bin"/*; do \
        tool="$(basename "$tool_path")"; \
        ln -sfn "$tool_path" "/opt/scarlet-rust-toolchain/lib/rustlib/${rust_host_triple}/bin/$tool"; \
        if [ ! -d "$tool_path" ]; then \
            ln -sfn "$tool_path" "/opt/scarlet-rust-toolchain/bin/$tool"; \
        fi; \
    done && \
    /opt/scarlet-rust-toolchain/bin/rustc --print target-list | grep -E '^(riscv64gc|aarch64)-unknown-scarlet$'

# Build xv6 and the user programs
RUN git clone https://github.com/mit-pdos/xv6-riscv.git /opt/xv6-riscv && \
    cd /opt/xv6-riscv && \
    git checkout 2a39c5af63906b3dbd0db58b9f6846ad70f4315d && \
    make fs.img

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
