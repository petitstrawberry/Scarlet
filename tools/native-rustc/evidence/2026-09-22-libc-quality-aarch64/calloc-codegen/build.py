#!/usr/bin/env python3
"""Run with a Scarlet rustc: RUSTC=/path/to/stage1/bin/rustc python3 build.py.
Emits before/after optimized LLVM IR from independent defining/calling crates.
"""
import os
from pathlib import Path
import subprocess

rustc = os.environ.get("RUSTC", "rustc")
root = Path(__file__).resolve().parent
common = [rustc, "--edition=2024", "--target", "aarch64-unknown-scarlet",
          "--crate-type=rlib", "-Copt-level=3"]
subprocess.run([rustc, "--version"], check=True)
for variant in ("baseline", "nobuiltins"):
    library = root / f"libdemo-{variant}.rlib"
    flags = ["--cfg", "disable_builtins"] if variant == "nobuiltins" else []
    subprocess.run(common + [str(root / "lib.rs"), "--crate-name=demo",
                   "-o", str(library)] + flags, check=True)
    subprocess.run(common + [str(root / "caller.rs"), "--crate-name=caller",
                   "--emit=llvm-ir", "--extern", f"demo={library}",
                   "-o", str(root / f"caller-{variant}.ll")], check=True)
