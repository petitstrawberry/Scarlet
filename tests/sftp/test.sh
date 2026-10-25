#!/usr/bin/env bash
# nix develop --command bash tests/sftp/test.sh
set -euo pipefail
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
out=$(mktemp -d "${TMPDIR:-/tmp}/scarlet-sftp-tests.XXXXXX")
trap 'rm -rf "$out"' EXIT
rustc --edition 2024 --test tests/sftp/stdio_server.rs -o "$out/protocol-tests"
"$out/protocol-tests"
rustc --edition 2024 tests/sftp/stdio_server.rs -o "$out/stdio-server"
python3 tests/sftp/openssh_smoke.py --artifacts "$out/interop" --server "$out/stdio-server"
