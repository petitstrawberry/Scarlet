#!/usr/bin/env python3
"""Run the repository AArch64 test runner with only CPU/accelerator overridden."""
from pathlib import Path
import os
import signal
import subprocess
import sys
import tempfile

root = Path(sys.argv[1]).resolve()
source = (root / "kernel/tools/test_aarch64.sh").read_text()
assert source.count("-cpu max") == 1
source = source.replace("-cpu max", "-cpu host -accel hvf")
print("Kernel validation override: -cpu host -accel hvf and HVF firmware", flush=True)
env = os.environ.copy()
for part in ("CODE", "VARS"):
    env[f"SCARLET_EFI_{part}_ARM64_EL2"] = env[f"SCARLET_EFI_{part}_ARM64_HVF"]
with tempfile.NamedTemporaryFile(mode="w", suffix=".sh", dir=root / "kernel/tools") as runner:
    runner.write(source)
    runner.flush()
    process = subprocess.Popen(["bash", runner.name, *sys.argv[2:]], start_new_session=True, env=env)
    try:
        status = process.wait(timeout=180)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        status = 124
raise SystemExit(status)
