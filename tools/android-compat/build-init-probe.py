#!/usr/bin/env python3
"""Build the bounded PID-namespace/init supervisor and userspace logging probe."""
import argparse
import json
from pathlib import Path
import platform
import shutil
import subprocess

from art_inputs import elf_needed, sha256

SOURCE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    host = {"Darwin": "darwin-x86_64", "Linux": "linux-x86_64"}[platform.system()]
    compiler = args.ndk.resolve() / "toolchains/llvm/prebuilt" / host / "bin/aarch64-linux-android33-clang"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    record = {"kind": "bounded-stock-init-probe-v1", "result": "PREPARING", "sources_sha256": {}, "files": {}, "commands": []}
    (output / "build.json").write_text(json.dumps(record) + "\n")
    for source, binary, options in [("init-scope-probe.c", "init-scope-probe", ["-static"]),
                                    ("init-log-probe.c", "libprobe-init-kmsg.so", ["-shared", "-fPIC", "-ldl"]),
                                    ("init-signal-helper.c", "init-signal-helper", ["-pie"]),
                                    ("android-runtime-shim.S", "libscarlet-android-shim.so",
                                     ["-shared", "-nostdlib"])]:
        shutil.copy2(SOURCE / source, output / source)
        language_options = ["-x", "assembler-with-cpp"] if source.endswith(".S") else ["-std=c11"]
        command = [str(compiler), *language_options, "-O2", "-g", "-Wall", "-Wextra", "-Werror",
                   str(output / source), *options, "-o", str(output / binary)]
        subprocess.run(command, check=True)
        record["commands"].append(command)
        record["sources_sha256"][source] = sha256(output / source)
        record["files"][binary] = sha256(output / binary)
    if elf_needed(output / "init-scope-probe"):
        raise ValueError("namespace supervisor must be static")
    record.update(result="BUILT", ndk=(args.ndk / "source.properties").read_text())
    (output / "build.json").write_text(json.dumps(record, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
