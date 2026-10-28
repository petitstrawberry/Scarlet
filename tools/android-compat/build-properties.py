#!/usr/bin/env python3
"""Build the ARM64 stock-format property snapshot helper (run in nix develop).

The serializer/parser sources are unmodified AOSP, pinned by tag AND SHA-256.
NDK static Bionic writes the property areas; NDK shared link stubs are never
installed in the ART runtime. This is a fixture bootstrap, not init/propertyd.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import urllib.request

from art_inputs import elf_needed, sha256


def build(ndk, cache, output, download=False):
    here = Path(__file__).resolve().parent
    sources = json.loads((here / "art/property-sources.json").read_text())
    version = (ndk / "source.properties").read_text()
    if "Pkg.Revision = 25.1.8937393" not in version:
        raise ValueError("expected NDK r25b (25.1.8937393)")
    for name, record in sources.items():
        target = cache / name
        if not target.exists() and download:
            target.parent.mkdir(parents=True, exist_ok=True)
            # Leave a failed download as an explicit error, never silently use
            # it or overwrite a previously supplied source.
            with urllib.request.urlopen(record["url"], timeout=60) as response, target.open("xb") as dest:
                shutil.copyfileobj(response, dest)
        if not target.is_file() or sha256(target) != record["sha256"]:
            raise ValueError(f"missing or mismatched pinned source: {target}; use --download for missing files")
    output.mkdir(parents=True, exist_ok=False)
    compiler = ndk / "toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android33-clang++"
    binary = output / "property-bootstrap"
    command = [str(compiler), "-std=c++17", "-static", "-O2", "-I", str(cache / "include"),
               str(here / "art/property-bootstrap.cpp")]
    command += [str(cache / name) for name in sources if name.endswith(".cpp")]
    command += ["-o", str(binary)]
    result = subprocess.run(command, capture_output=True, text=True)
    (output / "build.log").write_text(result.stdout + result.stderr)
    result.check_returncode()
    if elf_needed(binary):
        raise ValueError("property helper must be static")
    metadata = {"result": "BUILT", "execution": "not tested", "ndk": version,
                "compiler": subprocess.check_output([str(compiler), "--version"], text=True),
                "argv": command, "sources": sources,
                "helper_source_sha256": sha256(here / "art/property-bootstrap.cpp"),
                "binary_sha256": sha256(binary)}
    (output / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk", type=Path, required=True)
    parser.add_argument("--source-cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--download", action="store_true", help="fetch missing pinned AOSP sources")
    args = parser.parse_args()
    build(args.ndk.resolve(), args.source_cache.resolve(), args.output.resolve(), args.download)
    print(args.output.resolve())


if __name__ == "__main__":
    main()
