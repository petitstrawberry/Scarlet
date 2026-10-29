#!/usr/bin/env python3
"""Build real Bionic ARM64 probes with a locally installed Android NDK."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

SOURCE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk", type=Path, required=True)
    parser.add_argument("--api", type=int, default=33)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.api < 29:
        parser.error("API 29 or newer is required for native ELF TLS")
    host = {"Darwin": "darwin-x86_64", "Linux": "linux-x86_64"}.get(platform.system())
    compiler = args.ndk.resolve() / "toolchains/llvm/prebuilt" / str(host) / "bin" / f"aarch64-linux-android{args.api}-clang"
    if not compiler.is_file():
        parser.error(f"NDK compiler missing: {compiler}")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    flags = [str(compiler), "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-fno-emulated-tls"]
    probe_sources = [str(SOURCE / name) for name in
                     ("bionic-probe.c", "binder-probe.c", "binder-object-probe.c", "credentials-probe.c", "seccomp-probe.c", "process-scope-probe.c", "mount-restrictions-probe.c", "mount-propagation-probe.c", "mount-namespace-probe.c")]
    commands = [
        flags + ["-static"] + probe_sources + ["-ldl", "-o", str(output / "bionic-static")],
        flags + probe_sources + ["-ldl", "-o", str(output / "bionic-dynamic")],
        flags + ["-fPIC", "-shared", str(SOURCE / "bionic-library.c"), "-o", str(output / "libbionic-probe.so")],
    ]
    for command in commands:
        subprocess.run(command, check=True)
    names = ["bionic-static", "bionic-dynamic", "libbionic-probe.so"]
    metadata = {"sources_sha256": {str(Path(name).name): hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in probe_sources}, "api": args.api, "ndk": (args.ndk / "source.properties").read_text(),
                "commands": commands,
                "sha256": {name: hashlib.sha256((output / name).read_bytes()).hexdigest() for name in names}}
    (output / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
