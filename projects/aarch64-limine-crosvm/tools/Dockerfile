FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential curl ca-certificates git pkg-config libcap-dev libssl-dev \
    clang libclang-dev protobuf-compiler python3 && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh && \
    sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.88.0
ENV PATH="/root/.cargo/bin:${PATH}"
WORKDIR /src
