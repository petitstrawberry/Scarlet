#!/usr/bin/env python3
"""Build an Android 13 probe against pinned headers and unmodified runtime DSOs."""
import argparse
import json
from pathlib import Path
import platform
import shutil
import subprocess

from art_inputs import elf_needed, sha256, verify_art_runtime
from libbinder_inputs import LIBRARIES, SOURCE, prepare_headers


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    runtime, output = args.runtime_root.resolve(), args.output.resolve()
    verify_art_runtime(runtime)
    runtime_identity = sha256(runtime / "art-runtime.json")
    libraries = {name: sha256(runtime / name) for name in LIBRARIES}
    host = {"Darwin": "darwin-x86_64", "Linux": "linux-x86_64"}.get(platform.system())
    compiler = args.ndk.resolve() / "toolchains/llvm/prebuilt" / str(host) / "bin/aarch64-linux-android33-clang++"
    if not compiler.is_file():
        parser.error(f"NDK compiler missing: {compiler}")
    output.mkdir(parents=True, exist_ok=False)
    manifest = output / "build.json"
    manifest.write_text('{"result":"PREPARING"}\n')
    source, headers = output / "libbinder-probe.cpp", output / "libbinder-headers.json"
    shutil.copyfile(SOURCE / source.name, source)
    shutil.copyfile(SOURCE / headers.name, headers)
    pins = prepare_headers(output / "headers", json.loads(headers.read_text()))
    # AOSP uses std::__1 and a platform libc++.so; the NDK defaults to
    # std::__ndk1/libc++_shared.so. Do not mix those C++ ABIs or replace the DSO.
    command = [str(compiler), "-std=c++17", "-O2", "-g", "-Wall", "-Wextra", "-Werror",
               "-fno-rtti", "-fno-exceptions", "-fno-emulated-tls",
               "-D_LIBCPP_ABI_NAMESPACE=__1", "-nostdlib++"]
    for name in sorted(pins["archives"]):
        command += ["-isystem", str(output / "headers" / name)]
    binary = output / "libbinder-probe"
    command += [str(source)]
    command += [str(runtime / name) for name in LIBRARIES]
    command += ["-o", str(binary)]
    subprocess.run(command, check=True)
    needed = elf_needed(binary)
    if "libbinder.so" not in needed or "libc++.so" not in needed or "libc++_shared.so" in needed:
        raise ValueError("probe did not link stock platform libraries")
    verify_art_runtime(runtime)
    if libraries != {name: sha256(runtime / name) for name in LIBRARIES} or \
            runtime_identity != sha256(runtime / "art-runtime.json"):
        raise ValueError("runtime changed during compilation")
    metadata = {"result": "BUILT", "kind": "stock-libbinder-probe-v1", "api": 33,
                "ndk": (args.ndk / "source.properties").read_text(), "command": command,
                "headers": pins, "headers_manifest_sha256": sha256(headers),
                "source_sha256": sha256(source),
                "runtime_manifest_sha256": runtime_identity,
                "runtime_libraries": libraries, "needed": needed, "binary_sha256": sha256(binary)}
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
