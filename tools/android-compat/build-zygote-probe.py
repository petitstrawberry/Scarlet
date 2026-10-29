#!/usr/bin/env python3
"""Build the control-socket probe for a verified stock Android framework."""
import argparse
import json
from pathlib import Path
import platform
import subprocess

from art_inputs import sha256
from framework_inputs import verify_framework_inputs
from zygote_inputs import SOURCE, header


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("ndk", "framework-inputs", "runtime-root", "stock-inputs", "output"):
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    metadata = verify_framework_inputs(args.framework_inputs, args.runtime_root, args.stock_inputs)
    host = {"Darwin": "darwin-x86_64", "Linux": "linux-x86_64"}[platform.system()]
    compiler = args.ndk / "toolchains/llvm/prebuilt" / host / "bin/aarch64-linux-android33-clang"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "zygote-config.h").write_text(header(metadata))
    command = [str(compiler), "-O2", "-Wall", "-Wextra", "-Werror", "-I", str(output),
               str(SOURCE / "zygote-probe.c"), "-o", str(output / "zygote-probe")]
    subprocess.run(command, check=True)
    record = {"kind": "stock-zygote-control-v2", "command": command,
              "source_sha256": sha256(SOURCE / "zygote-probe.c"),
              "framework_sha256": sha256(args.framework_inputs / "framework-inputs.json"),
              "program_sha256": sha256(output / "zygote-probe")}
    (output / "build.json").write_text(json.dumps(record, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
