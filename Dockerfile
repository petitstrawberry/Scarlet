FROM nixos/nix:latest AS base

ARG SCARLET_CACHIX_CACHE=""
ARG SCARLET_CACHIX_PUBLIC_KEY=""

RUN mkdir -p /etc/nix && \
    printf '%s\n' \
      'experimental-features = nix-command flakes' \
      'accept-flake-config = true' \
      'sandbox = false' \
      >> /etc/nix/nix.conf

RUN if [ -n "${SCARLET_CACHIX_CACHE}" ] && [ -n "${SCARLET_CACHIX_PUBLIC_KEY}" ]; then \
        printf '%s\n' \
          "extra-substituters = https://${SCARLET_CACHIX_CACHE}.cachix.org" \
          "extra-trusted-public-keys = ${SCARLET_CACHIX_PUBLIC_KEY}" \
          >> /etc/nix/nix.conf; \
    fi

WORKDIR /workspaces/Scarlet

COPY docker/entrypoint.sh /usr/local/bin/scarlet-dev-entrypoint
RUN chmod +x /usr/local/bin/scarlet-dev-entrypoint && \
    mkdir -p /opt/prebuilt

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
ENV SCARLET_RUST_TARGET_TRIPLES="riscv64gc-unknown-scarlet aarch64-unknown-scarlet"

ENTRYPOINT ["/usr/local/bin/scarlet-dev-entrypoint"]
CMD ["bash"]

FROM base AS ci-build

ARG SCARLET_RUST_REPOSITORY="https://github.com/petitstrawberry/rust.git"
ARG SCARLET_RUST_REV=""

COPY flake.nix flake.lock ./
COPY nix ./nix
COPY scripts ./scripts

RUN test -n "${SCARLET_RUST_REV}" || (echo "SCARLET_RUST_REV is required for the ci image" >&2; exit 1)

RUN nix shell nixpkgs#git --command sh -c '\
        git init rust && \
        git -C rust remote add origin "${SCARLET_RUST_REPOSITORY}" && \
        git -C rust fetch --depth 1 origin "${SCARLET_RUST_REV}" && \
        git -C rust checkout --detach FETCH_HEAD && \
        git -C rust submodule update --init --recursive --depth 1 \
    '

RUN nix develop .#default --command true && \
    mkdir -p /opt && \
    mv rust/build /opt/scarlet-rust-build

FROM ci-build AS ci

FROM base AS dev
