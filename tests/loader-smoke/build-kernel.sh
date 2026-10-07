#!/usr/bin/env bash
# Build a disposable Limine BSP against this checkout's kernel sources.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUTPUT=""
SMOKE_ARCH=aarch64
CARGO_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            [[ $# -ge 2 ]] || { echo "Missing --arch value" >&2; exit 2; }
            SMOKE_ARCH="$2"
            shift 2
            ;;
        --output)
            [[ $# -ge 2 ]] || { echo 'Missing --output directory' >&2; exit 2; }
            OUTPUT="$2"
            shift 2
            ;;
        --offline)
            CARGO_ARGS+=(--offline)
            shift
            ;;
        -h|--help)
            echo 'Usage: build-kernel.sh [--arch aarch64|riscv64] [--output DIRECTORY] [--offline]'
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 2
            ;;
    esac
done
case "$SMOKE_ARCH" in
    aarch64) SMOKE_TARGET=aarch64-unknown-none-elf ;;
    riscv64) SMOKE_TARGET=riscv64gc-unknown-none-elf ;;
    *) echo "Unsupported architecture: $SMOKE_ARCH" >&2; exit 2 ;;
esac
if [[ -z "$OUTPUT" ]]; then
    OUTPUT="$REPO_ROOT/target/loader-smoke/kernel"
    [[ "$SMOKE_ARCH" == aarch64 ]] || OUTPUT="$OUTPUT-$SMOKE_ARCH"
fi
OUTPUT="$(python3 -c 'import pathlib, sys; print(pathlib.Path(sys.argv[1]).resolve())' "$OUTPUT")"
python3 - "$REPO_ROOT" "$OUTPUT" "$SMOKE_ARCH" <<'PY'
import json
from pathlib import Path
import sys

repo, output = map(Path, sys.argv[1:3])
arch = sys.argv[3]
project = "aarch64-limine-console" if arch == "aarch64" else "riscv64-limine-full"
bsp = output / 'bsp'
(bsp / 'src').mkdir(parents=True, exist_ok=True)
(bsp / 'lds').mkdir(exist_ok=True)
(bsp / 'Cargo.toml').write_text('''[package]
name = "scarlet-loader-smoke-kernel"
version = "0.1.0"
edition = "2024"
[workspace]
[dependencies]
scarlet = { path = ''' + json.dumps(str(repo / 'kernel')) + ''', default-features = false, features = ["network", "user-fpu", "user-vector", "limine"] }
[profile.dev]
opt-level = 3
panic = "abort"
''')
source = (repo / 'projects' / project / 'bsp/src/main.rs').read_text()
source = source.replace('scarlet_modules::scarlet::', 'scarlet::')
source = source.replace('extern crate scarlet_modules;', 'extern crate scarlet;')
source = source.replace('    scarlet_modules::force_link();\n', '')
(bsp / 'src/main.rs').write_text(source)
script = f'{arch}_limine.ld'
(bsp / 'lds' / script).write_text((repo / 'kernel/lds' / script).read_text())
PY
cd "$OUTPUT/bsp"
CARGO_TARGET_DIR="$OUTPUT/target" \
RUSTFLAGS="${RUSTFLAGS:-} -C no-vectorize-loops -C no-vectorize-slp" \
cargo build "${CARGO_ARGS[@]}" \
    --target "$REPO_ROOT/kernel/targets/$SMOKE_TARGET.json" \
    -Z build-std=core,alloc,compiler_builtins \
    -Z build-std-features=compiler-builtins-mem
printf '%s\n' "$OUTPUT/target/$SMOKE_TARGET/debug/scarlet-loader-smoke-kernel"
