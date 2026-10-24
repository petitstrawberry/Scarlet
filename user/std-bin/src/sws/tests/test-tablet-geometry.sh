#!/usr/bin/env bash
# Usage: nix develop --command bash user/std-bin/src/sws/tests/test-tablet-geometry.sh
# Uses the actual OS wrapper crates, but runs only tests that make no OS calls.
set -euo pipefail
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
out=$(mktemp -d "${TMPDIR:-/tmp}/sws-tablet-tests.XXXXXX")
trap 'rm -rf "$out"' EXIT
rustc --edition=2024 --crate-type=rlib --crate-name sws_protocol --cfg 'feature="std"' user/lib/sws-protocol/src/lib.rs --out-dir "$out"
rustc --edition=2024 --crate-type=rlib --crate-name scarlet_abi user/lib/scarlet-abi/src/lib.rs --out-dir "$out"
rustc --edition=2024 --crate-type=rlib --crate-name scarlet_sys user/lib/scarlet-sys/src/lib.rs --extern scarlet_abi="$out/libscarlet_abi.rlib" -L "$out" --out-dir "$out"
rustc --edition=2024 --crate-type=rlib --crate-name scarlet_os --cfg 'feature="std"' user/lib/scarlet-os/src/lib.rs --extern scarlet_abi="$out/libscarlet_abi.rlib" --extern scarlet_sys="$out/libscarlet_sys.rlib" -L "$out" --out-dir "$out"
rustc --edition=2024 --test user/std-bin/src/sws/tests/tablet_geometry.rs --extern sws_protocol="$out/libsws_protocol.rlib" --extern scarlet_os="$out/libscarlet_os.rlib" -L "$out" -A dead_code -o "$out/tests"
"$out/tests" tablet_
"$out/tests" fullscreen_
"$out/tests" frame_callback::tests::
"$out/tests" window::rounded_geometry_tests::
"$out/tests" input_panel::tests::
"$out/tests" workspace_geometry::tests::
"$out/tests" input::tests::
rustc --edition=2024 --test --crate-name sws_protocol --cfg 'feature="std"' user/lib/sws-protocol/src/lib.rs -o "$out/protocol-tests"
"$out/protocol-tests"
