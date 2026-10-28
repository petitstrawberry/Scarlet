#!/usr/bin/env python3
"""Run the production shell network policy with the actual link ABI on the host."""
from pathlib import Path
import json
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="scarlet-shell-network-") as temporary:
    work = Path(temporary)
    harness = work / "tests.rs"
    harness.write_text(
        'extern crate alloc;\nextern crate self as scarlet_abi;\n'
        + f'#[path = {json.dumps(str(ROOT / "user/lib/scarlet-abi/src/network.rs"))}]\n'
        + 'pub mod network;\n'
        + f'#[path = {json.dumps(str(ROOT / "user/std-bin/src/scarlet_shell/network_status.rs"))}]\n'
        + 'mod network_status;\n'
    )
    binary = work / "shell-network-tests"
    subprocess.run(["rustc", "--edition=2024", "--test", str(harness), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
