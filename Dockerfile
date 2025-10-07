FROM ubuntu:24.04

ENV PATH=/root/.cargo/bin:$PATH
ENV MAKEFLAGS=-j$(($(nproc)-2))
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

ENV DEBIAN_FRONTEND noninteractive

# Install dependencies and tools
RUN apt update && \
	apt install -y build-essential autoconf automake autotools-dev curl bc git device-tree-compiler vim python3 gdb-multiarch gcc-riscv64-linux-gnu cpio \
  mtools dosfstools sleuthkit

# Install QEMU
RUN apt install -y qemu-system-riscv64 qemu-system-arm

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

WORKDIR /workspaces/Scarlet