_setup_scarlet_rust_toolchain() {
    set -e
    if [ -n "${ZSH_VERSION:-}" ]; then
        setopt local_options sh_word_split typeset_silent
    fi

    local repo_root
    repo_root="${SCARLET_REPO_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"

    local host_triple="${SCARLET_RUST_HOST_TRIPLE:-}"
    if [ -z "${host_triple}" ] && command -v rustc >/dev/null 2>&1; then
        host_triple="$(rustc -vV | sed -n 's/^host: //p' | head -n 1)"
    fi
    if [ -z "${host_triple}" ]; then
        case "$(uname -m)-$(uname -s)" in
            x86_64-Linux) host_triple="x86_64-unknown-linux-gnu" ;;
            aarch64-Linux | arm64-Linux) host_triple="aarch64-unknown-linux-gnu" ;;
            x86_64-Darwin) host_triple="x86_64-apple-darwin" ;;
            arm64-Darwin | aarch64-Darwin) host_triple="aarch64-apple-darwin" ;;
        esac
    fi
    if [ -z "${host_triple}" ]; then
        echo "Could not detect the Rust host triple. Set SCARLET_RUST_HOST_TRIPLE." >&2
        return 1
    fi

    local rust_dir="${repo_root}/rust"
    local stage_dir="${rust_dir}/build/${host_triple}/stage1"
    local stage0_dir="${rust_dir}/build/${host_triple}/stage0"
    local stage0_sysroot_dir="${rust_dir}/build/${host_triple}/stage0-sysroot"
    local target_triples="${SCARLET_RUST_TARGET_TRIPLES:-riscv64gc-unknown-scarlet aarch64-unknown-scarlet}"

    if [ ! -x "${stage_dir}/bin/rustc" ]; then
        (
            cd "${rust_dir}"
            ./x build compiler/rustc
        )
    fi

    local cxx_var="CXX_$(printf '%s' "${host_triple}" | tr '-' '_')"
    local default_cxx="c++"
    case "${host_triple}" in
        *-apple-darwin) default_cxx="clang++" ;;
    esac
    local current_cxx=""
    eval "current_cxx=\"\${${cxx_var}:-}\""

    _rust_toolchain_libs_missing() {
        local target_triple="$1"
        local target_lib_dir="${stage_dir}/lib/rustlib/${target_triple}/lib"

        if ! find "${target_lib_dir}" -maxdepth 1 -name 'libstd-*.rlib' -type f 2>/dev/null | grep -q .; then
            return 0
        fi

        if ! find "${target_lib_dir}" -maxdepth 1 -name 'libproc_macro-*.rlib' -type f 2>/dev/null | grep -q .; then
            return 0
        fi

        if ! find "${target_lib_dir}" -maxdepth 1 -name 'libtest-*.rlib' -type f 2>/dev/null | grep -q .; then
            return 0
        fi

        return 1
    }

    local build_targets="${host_triple}"
    local needs_lib_build=0
    if _rust_toolchain_libs_missing "${host_triple}"; then
        needs_lib_build=1
    fi
    local target_triple
    for target_triple in ${target_triples}; do
        build_targets="${build_targets},${target_triple}"
        if _rust_toolchain_libs_missing "${target_triple}"; then
            needs_lib_build=1
        fi
    done

    if [ "${needs_lib_build}" -eq 1 ]; then
        echo "Preparing Rust library sysroot for ${build_targets}..."
        (
            cd "${rust_dir}"
            env "${cxx_var}=${current_cxx:-${default_cxx}}" ./x build library --target "${build_targets}"
        )
    fi

    case ":${PATH}:" in
        *":${stage_dir}/bin:"*) ;;
        *) export PATH="${stage_dir}/bin:${PATH}" ;;
    esac
    hash -r 2>/dev/null || true

    for rust_lld_dir in \
        "${stage_dir}/lib/rustlib/${host_triple}/bin" \
        "${stage0_dir}/lib/rustlib/${host_triple}/bin" \
        "${stage0_sysroot_dir}/lib/rustlib/${host_triple}/bin"
    do
        if [ -x "${rust_lld_dir}/rust-lld" ]; then
            case ":${PATH}:" in
                *":${rust_lld_dir}:"*) ;;
                *) export PATH="${rust_lld_dir}:${PATH}" ;;
            esac
            break
        fi
    done

    export SCARLET_RUST_HOST_TRIPLE="${host_triple}"
    export SCARLET_RUST_TARGET_TRIPLES="${target_triples}"
    export SCARLET_RUST_TOOLCHAIN="${stage_dir}"

    unset -f _rust_toolchain_libs_missing
}

_setup_scarlet_rust_toolchain
unset -f _setup_scarlet_rust_toolchain
