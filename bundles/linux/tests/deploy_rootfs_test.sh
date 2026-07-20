#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY_ROOTFS_SCRIPT="${SCRIPT_DIR}/../tools/deploy_rootfs.sh"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/scarlet-linux-deploy-rootfs.XXXXXX")"
trap 'rm -rf "${TEST_DIR}"' EXIT

assert_file_contents() {
    local expected="$1"
    local file="$2"
    local expected_file="${TEST_DIR}/expected"

    printf '%s\n' "$expected" > "$expected_file"
    cmp -s "$expected_file" "$file"
}

run_deploy_case() {
    local arch="$1"
    local case_dir="${TEST_DIR}/${arch}"
    local project_root="${case_dir}/project"
    local prebuilt_dir="${case_dir}/prebuilt"
    local rootfs_fixture="${case_dir}/rootfs-fixture"
    local dest_dir="${project_root}/bundles/linux/rootfs/system/linux-${arch}"

    mkdir -p \
        "${rootfs_fixture}/etc" \
        "${rootfs_fixture}/usr/bin" \
        "${prebuilt_dir}/${arch}/root/etc" \
        "${prebuilt_dir}/${arch}/bin" \
        "${prebuilt_dir}/${arch}/lib" \
        "${prebuilt_dir}/${arch}/share/fixture"

    printf '%s\n' 'from-rootfs' > "${rootfs_fixture}/etc/rootfs-only"
    printf '%s\n' 'from-rootfs' > "${rootfs_fixture}/etc/motd"
    printf '%s\n' 'base executable' > "${rootfs_fixture}/usr/bin/base-tool"
    tar -cf "${prebuilt_dir}/${arch}/rootfs.tar" -C "${rootfs_fixture}" .

    printf '%s\n' 'from-overlay' > "${prebuilt_dir}/${arch}/root/etc/motd"
    printf '%s\n' 'overlay-only' > "${prebuilt_dir}/${arch}/root/etc/overlay-only"
    printf '%s\n' 'staged executable' > "${prebuilt_dir}/${arch}/bin/staged-tool"
    chmod 0644 "${prebuilt_dir}/${arch}/bin/staged-tool"
    printf '%s\n' 'staged library' > "${prebuilt_dir}/${arch}/lib/libfixture.so"
    printf '%s\n' 'staged data' > "${prebuilt_dir}/${arch}/share/fixture/data.txt"

    mkdir -p "${dest_dir}"
    printf '%s\n' 'stale' > "${dest_dir}/.stale"

    ARCH="$arch" \
    PREBUILT_DIR="$prebuilt_dir" \
    PROJECT_ROOT="$project_root" \
    bash "$DEPLOY_ROOTFS_SCRIPT"

    assert_file_contents 'from-rootfs' "${dest_dir}/etc/rootfs-only"
    assert_file_contents 'from-overlay' "${dest_dir}/etc/motd"
    assert_file_contents 'overlay-only' "${dest_dir}/etc/overlay-only"
    assert_file_contents 'base executable' "${dest_dir}/usr/bin/base-tool"
    assert_file_contents 'staged executable' "${dest_dir}/usr/bin/staged-tool"
    assert_file_contents 'staged library' "${dest_dir}/usr/lib/libfixture.so"
    assert_file_contents 'staged data' "${dest_dir}/usr/share/fixture/data.txt"

    test -x "${dest_dir}/usr/bin/staged-tool"
    test ! -e "${dest_dir}/.stale"
}

run_deploy_case aarch64
run_deploy_case riscv64

guard_prebuilt="${TEST_DIR}/aarch64/prebuilt"
guard_victim="${TEST_DIR}/guard-victim"
mkdir -p "$guard_victim"
printf '%s\n' 'preserve-me' > "${guard_victim}/sentinel"
if ARCH=aarch64 PREBUILT_DIR="$guard_prebuilt" PROJECT_ROOT="${TEST_DIR}/guard-project" \
    ROOTFS_DEST_DIR="$guard_victim" bash "$DEPLOY_ROOTFS_SCRIPT" >/dev/null 2>&1; then
    echo "deploy_rootfs_test: unapproved destination override unexpectedly succeeded" >&2
    exit 1
fi
assert_file_contents 'preserve-me' "${guard_victim}/sentinel"

symlink_project="${TEST_DIR}/symlink-project"
symlink_victim="${TEST_DIR}/symlink-victim"
mkdir -p "${symlink_project}/bundles/linux/rootfs/system" "$symlink_victim"
printf '%s\n' 'preserve-me' > "${symlink_victim}/sentinel"
ln -s "$symlink_victim" "${symlink_project}/bundles/linux/rootfs/system/linux-aarch64"
if ARCH=aarch64 PREBUILT_DIR="$guard_prebuilt" PROJECT_ROOT="$symlink_project" \
    bash "$DEPLOY_ROOTFS_SCRIPT" >/dev/null 2>&1; then
    echo "deploy_rootfs_test: symlink destination unexpectedly succeeded" >&2
    exit 1
fi
assert_file_contents 'preserve-me' "${symlink_victim}/sentinel"

if ARCH=x86_64 PREBUILT_DIR="${TEST_DIR}/invalid" PROJECT_ROOT="${TEST_DIR}/invalid-project" \
    bash "$DEPLOY_ROOTFS_SCRIPT" >/dev/null 2>&1; then
    echo "deploy_rootfs_test: unsupported architecture unexpectedly succeeded" >&2
    exit 1
fi

printf '%s\n' 'deploy_rootfs_test: PASS'
