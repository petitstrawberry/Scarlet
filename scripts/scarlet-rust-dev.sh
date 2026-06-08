#!/usr/bin/env sh

_scarlet_rust_path_remove() {
    remove_path="$1"
    new_path=""
    old_ifs="$IFS"
    IFS=:
    for path_part in $PATH; do
        if [ "${path_part}" = "${remove_path}" ]; then
            continue
        fi
        if [ -z "${new_path}" ]; then
            new_path="${path_part}"
        else
            new_path="${new_path}:${path_part}"
        fi
    done
    IFS="$old_ifs"
    PATH="$new_path"
}

_scarlet_rust_path_prepend() {
    add_path="$1"
    _scarlet_rust_path_remove "$add_path"
    PATH="${add_path}:${PATH}"
    export PATH
}

_scarlet_rust_stage1_dir() {
    rust_dir="$1"
    host_triple="${SCARLET_RUST_HOST_TRIPLE:-}"
    if [ -z "${host_triple}" ]; then
        host_triple="$(rustc -vV | sed -n 's/^host: //p' | head -n 1)"
    fi
    printf '%s\n' "${rust_dir}/build/${host_triple}/stage1"
}

scarlet-rust-use-cached() {
    if [ -z "${SCARLET_CACHED_RUST_TOOLCHAIN:-}" ]; then
        echo "SCARLET_CACHED_RUST_TOOLCHAIN is not set." >&2
        return 1
    fi

    if [ -n "${SCARLET_RUST_ACTIVE_BIN:-}" ]; then
        _scarlet_rust_path_remove "${SCARLET_RUST_ACTIVE_BIN}"
    fi

    SCARLET_RUST_ACTIVE_BIN="${SCARLET_CACHED_RUST_TOOLCHAIN}/bin"
    SCARLET_RUST_TOOLCHAIN="${SCARLET_CACHED_RUST_TOOLCHAIN}"
    unset RUSTC
    unset RUSTDOC
    _scarlet_rust_path_prepend "${SCARLET_RUST_ACTIVE_BIN}"
    export SCARLET_RUST_ACTIVE_BIN SCARLET_RUST_TOOLCHAIN

    echo "Using cached Scarlet Rust toolchain: ${SCARLET_RUST_TOOLCHAIN}"
}

scarlet-rust-use-local() {
    rust_dir="${1:-${SCARLET_RUST_LOCAL_DIR:-}}"
    if [ -z "${rust_dir}" ]; then
        echo "Usage: scarlet-rust-use-local /path/to/rust-fork" >&2
        echo "Or set SCARLET_RUST_LOCAL_DIR=/path/to/rust-fork." >&2
        return 2
    fi

    if [ ! -d "${rust_dir}" ]; then
        echo "Rust fork directory does not exist: ${rust_dir}" >&2
        return 1
    fi

    stage1="$(_scarlet_rust_stage1_dir "${rust_dir}")"
    if [ ! -x "${stage1}/bin/rustc" ]; then
        echo "stage1 rustc is missing: ${stage1}/bin/rustc" >&2
        echo "Build it in the Rust fork first, for example:" >&2
        echo "  ./x build compiler/rustc library/std --target ${SCARLET_RUST_TARGET_TRIPLES:-riscv64gc-unknown-scarlet}" >&2
        return 1
    fi

    if [ -n "${SCARLET_RUST_ACTIVE_BIN:-}" ]; then
        _scarlet_rust_path_remove "${SCARLET_RUST_ACTIVE_BIN}"
    fi

    SCARLET_RUST_LOCAL_DIR="${rust_dir}"
    SCARLET_RUST_ACTIVE_BIN="${stage1}/bin"
    SCARLET_RUST_TOOLCHAIN="${stage1}"
    RUSTC="${stage1}/bin/rustc"
    if [ -x "${stage1}/bin/rustdoc" ]; then
        RUSTDOC="${stage1}/bin/rustdoc"
        export RUSTDOC
    else
        unset RUSTDOC
    fi

    _scarlet_rust_path_prepend "${SCARLET_RUST_ACTIVE_BIN}"
    export SCARLET_RUST_LOCAL_DIR SCARLET_RUST_ACTIVE_BIN SCARLET_RUST_TOOLCHAIN RUSTC

    echo "Using local Scarlet Rust stage1: ${SCARLET_RUST_TOOLCHAIN}"
}

scarlet-rust-show() {
    echo "SCARLET_RUST_TOOLCHAIN=${SCARLET_RUST_TOOLCHAIN:-}"
    echo "SCARLET_CACHED_RUST_TOOLCHAIN=${SCARLET_CACHED_RUST_TOOLCHAIN:-}"
    echo "SCARLET_RUST_LOCAL_DIR=${SCARLET_RUST_LOCAL_DIR:-}"
    echo "RUSTC=${RUSTC:-$(command -v rustc 2>/dev/null || true)}"
    rustc -vV 2>/dev/null || true
}

if [ -n "${SCARLET_CACHED_RUST_TOOLCHAIN:-}" ]; then
    if [ -z "${SCARLET_RUST_ACTIVE_BIN:-}" ]; then
        SCARLET_RUST_ACTIVE_BIN="${SCARLET_CACHED_RUST_TOOLCHAIN}/bin"
    fi
    if [ -z "${SCARLET_RUST_TOOLCHAIN:-}" ]; then
        SCARLET_RUST_TOOLCHAIN="${SCARLET_CACHED_RUST_TOOLCHAIN}"
    fi
    _scarlet_rust_path_prepend "${SCARLET_RUST_ACTIVE_BIN}"
    export SCARLET_RUST_ACTIVE_BIN SCARLET_RUST_TOOLCHAIN
fi
