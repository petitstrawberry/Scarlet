FROM ubuntu:24.04

ENV PATH=/root/.cargo/bin:$PATH
ENV MAKEFLAGS=-j$(nproc-2)
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

ENV DEBIAN_FRONTEND noninteractive

# Install dependencies and tools
RUN apt update && \
	apt install -y build-essential autoconf automake autotools-dev curl bc git device-tree-compiler vim python3 gdb-multiarch gcc-riscv64-linux-gnu

# Install QEMU
RUN apt install -y qemu-system-riscv64

# Install Rust and RISC-V target
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y && \
    rustup install nightly && \
    rustup component add rust-src --toolchain nightly && \
    rustup target add riscv64gc-unknown-none-elf

WORKDIR /workspaces/Scarlet