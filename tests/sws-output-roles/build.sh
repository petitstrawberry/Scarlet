#!/usr/bin/env bash
# Run in the Scarlet dev shell. Usage: build.sh SCRATCH AARCH64_FULL_PROJECT
# The project supplies its normal generated SGFX config and staged loader/init.
# All builds and image writes go into SCRATCH. Reuse it between sequential runs.
# Then run: python3 tests/sws-output-roles/run.py "$scratch/boot.img" "$scratch/run"
# Use --accel hvf on Apple Silicon. For a real VirGL run, repeat with
# --backend sgfx and a fresh run directory. On macOS, the Cocoa capture helper
# uses Xcode Swift, Screen Recording and Accessibility to inspect QEMU's window.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
mkdir -p "${1:?supply a private scratch directory}"
scratch=$(cd "$1" && pwd)
project=$(cd "${2:?supply a built aarch64-limine-full project directory}" && pwd)
cd "$repo"
stage="$scratch/staging"
target=aarch64-unknown-scarlet
export CARGO_TARGET_DIR="$scratch/target"
cargo build --locked --release --manifest-path .cargo/Cargo.toml --target "$target" \
    --config "$project/.scarlet/sgfx/userspace.toml" -p scarlet-std-bin --bin sws --bin displayctl
cargo build --locked --release --manifest-path "$here/Cargo.toml" --target "$target"
"$repo/tests/loader-smoke/build-kernel.sh" --output "$scratch/kernel"
kernel="$scratch/kernel/target/aarch64-unknown-none-elf/debug/scarlet-loader-smoke-kernel"
llvm-objcopy --strip-debug "$kernel" "$scratch/kernel.elf"
mkdir -p "$stage"/{bin,dev/pts,etc/sws,lib/sgfx,mnt/newroot,root,tmp}
base="$project/.scarlet/staging"
cp "$base/initramfs/init" "$stage/init"
cp "$base/rootfs/bin/scarlet-ld" "$stage/bin/"
cp "$base"/rootfs/lib/sgfx/{libsgfx_scarlet_virgl.so,scarlet-virgl.sgfx-driver} "$stage/lib/sgfx/"
cp "$CARGO_TARGET_DIR/$target/release/"{sws,displayctl,sws-output-roles} "$stage/bin/"
printf '[output]\nscale = 1.0\n' > "$stage/etc/sws/config.toml"
python3 - "$repo" "$stage" "$scratch" <<'PY'
from pathlib import Path
import runpy, sys
repo, stage, scratch = map(Path, sys.argv[1:])
create = runpy.run_path(str(repo / "tests/loader-smoke/run-qemu.py"))["create_initramfs"]
create(stage, scratch / "initramfs.cpio")
PY
cargo-scarlet-plugin-limine --arch aarch64 --kernel "$scratch/kernel.elf" \
    --initramfs "$scratch/initramfs.cpio" --output "$scratch/boot.img" \
    --cache-dir "$scratch/limine-cache" \
    --cmdline 'console=ttyAMA0 init=/init init.console=/dev/tty0 init.exec=/bin/sws-output-roles'
