FROM nixos/nix:latest AS base

ARG SCARLET_CACHIX_CACHE="scarlet-rust-toolchain"
ARG SCARLET_CACHIX_PUBLIC_KEY="scarlet-rust-toolchain.cachix.org-1:p+coBExi0nNTIvWF/oM9H9/1/GhwFtqGZ2Vs+4pYl6o="

RUN mkdir -p /etc/nix && \
    printf '%s\n' \
      'experimental-features = nix-command flakes' \
      'accept-flake-config = true' \
      'sandbox = false' \
      >> /etc/nix/nix.conf

RUN cache="${SCARLET_CACHIX_CACHE:-scarlet-rust-toolchain}"; \
    public_key="${SCARLET_CACHIX_PUBLIC_KEY:-scarlet-rust-toolchain.cachix.org-1:p+coBExi0nNTIvWF/oM9H9/1/GhwFtqGZ2Vs+4pYl6o=}"; \
    if [ -n "${cache}" ] && [ -n "${public_key}" ]; then \
        printf '%s\n' \
          "extra-substituters = https://${cache}.cachix.org" \
          "extra-trusted-public-keys = ${public_key}" \
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
