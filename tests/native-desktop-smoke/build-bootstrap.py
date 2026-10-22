#!/usr/bin/env python3
"""Build the existing bootstrap init/SBUS daemon plus the disposable fixture setup."""
import argparse
import json
from pathlib import Path
import subprocess

REPO = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, required=True, help="scratch Cargo project directory")
args = parser.parse_args()
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True)
quote = lambda path: json.dumps(str(path))
(output / "Cargo.toml").write_text(
    '[package]\nname = "scarlet-native-smoke-bootstrap"\nversion = "0.1.0"\nedition = "2024"\n'
    '[workspace]\n[dependencies]\n'
    f'scarlet-std = {{ path = {quote(REPO / "user/lib/std")} }}\n'
    f'sbus = {{ path = {quote(REPO / "user/lib/sbus")} }}\n'
    '[[bin]]\nname = "native-smoke-init"\n'
    f'path = {quote(REPO / "user/bin/src/init.rs")}\n'
    '[[bin]]\nname = "sbusd"\n'
    f'path = {quote(REPO / "user/bin/src/sbusd/main.rs")}\n'
    '[[bin]]\nname = "prepare-desktop"\n'
    f'path = {quote(REPO / "tests/native-desktop-smoke/prepare.rs")}\n'
    '[profile.release]\npanic = "abort"\n'
)
subprocess.run(["cargo", "build", "--release", "--manifest-path", str(output / "Cargo.toml"),
                "--target", "aarch64-unknown-scarlet"], check=True)
