FROM nixos/nix:latest AS base

ARG SCARLET_RUST_TOOLCHAIN_CACHIX_CACHE="scarlet-rust-toolchain"
ARG SCARLET_RUST_TOOLCHAIN_CACHIX_PUBLIC_KEY="scarlet-rust-toolchain.cachix.org-1:p+coBExi0nNTIvWF/oM9H9/1/GhwFtqGZ2Vs+4pYl6o="
ARG SCARLET_CACHIX_CACHE=""
ARG SCARLET_CACHIX_PUBLIC_KEY=""

RUN mkdir -p /etc/nix && \
    printf '%s\n' \
      'experimental-features = nix-command flakes' \
      'accept-flake-config = true' \
      'sandbox = false' \
      >> /etc/nix/nix.conf

RUN substituters="https://${SCARLET_RUST_TOOLCHAIN_CACHIX_CACHE}.cachix.org"; \
    public_keys="${SCARLET_RUST_TOOLCHAIN_CACHIX_PUBLIC_KEY}"; \
    if [ -n "${SCARLET_CACHIX_CACHE}" ] && [ "${SCARLET_CACHIX_CACHE}" != "${SCARLET_RUST_TOOLCHAIN_CACHIX_CACHE}" ]; then \
        substituters="${substituters} https://${SCARLET_CACHIX_CACHE}.cachix.org"; \
    fi; \
    if [ -n "${SCARLET_CACHIX_PUBLIC_KEY}" ] && [ "${SCARLET_CACHIX_PUBLIC_KEY}" != "${SCARLET_RUST_TOOLCHAIN_CACHIX_PUBLIC_KEY}" ]; then \
        public_keys="${public_keys} ${SCARLET_CACHIX_PUBLIC_KEY}"; \
    fi; \
    if [ -n "${substituters}" ] && [ -n "${public_keys}" ]; then \
        printf '%s\n' \
          "extra-substituters = ${substituters}" \
          "extra-trusted-public-keys = ${public_keys}" \
          >> /etc/nix/nix.conf; \
    fi

WORKDIR /workspaces/Scarlet

COPY flake.nix flake.lock ./
COPY nix ./nix

RUN nix develop .#default --command true

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
ENV SCARLET_RUST_TARGET_TRIPLES="riscv64gc-unknown-scarlet aarch64-unknown-scarlet"

ENTRYPOINT ["nix", "develop", ".#default", "--command"]
CMD ["bash"]

FROM base AS ci

FROM base AS dev
