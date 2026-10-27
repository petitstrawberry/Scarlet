#!/usr/bin/env python3
"""Test the production CLI status parser without a kernel or radio device."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="wireless-cli-") as temporary:
    binary = Path(temporary) / "tests"
    subprocess.run(["rustc", "--edition=2024", "--test", str(ROOT / "user/wpa-supplicant/src/status.rs"), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
