#!/usr/bin/env python3
"""Build the shared SWS client and a native-transport probe for Android/Bionic."""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess

from art_inputs import elf_needed, sha256

SOURCE = Path(__file__).resolve().parent
ROOT = SOURCE.parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ndk", "rust-toolchain", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--sysroot", type=Path, help="optional local Rust sysroot containing the Android standard library")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    toolchain = args.rust_toolchain.resolve() / "bin"
    host = {"Darwin": "darwin-x86_64", "Linux": "linux-x86_64"}[platform.system()]
    compiler = args.ndk.resolve() / "toolchains/llvm/prebuilt" / host / "bin/aarch64-linux-android33-clang"
    environment = os.environ.copy()
    environment.update(RUSTC=str(toolchain / "rustc"),
                       CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=str(compiler))
    rustflags = ["--sysroot", str(args.sysroot.resolve())] if args.sysroot else []
    # Keep the cross-target flags separate from host build scripts.
    environment.pop("RUSTFLAGS", None)
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(rustflags)
    commands = [[str(toolchain / "cargo"), "build", "--manifest-path", str(ROOT / "user/lib/sws-client-c/Cargo.toml"),
                 "--target", "aarch64-linux-android", "--release", "--target-dir", str(output / "target")]]
    subprocess.run(commands[-1], check=True, env=environment, cwd=ROOT)
    release = output / "target/aarch64-linux-android/release"
    shared = output / "libsws_client_c.so"
    shutil.copy2(release / shared.name, shared)
    commands.append([str(toolchain / "rustc"), "--edition", "2024", "--target", "aarch64-linux-android",
                     *rustflags, "--crate-type", "staticlib", "-C", "panic=abort", "-C", "opt-level=2",
                     "-L", "dependency=" + str(release / "deps")])
    for name in ("scarlet_abi", "scarlet_sys"):
        candidates = list((release / "deps").glob("lib" + name + "-*.rlib"))
        if len(candidates) != 1:
            raise ValueError("ambiguous SDK dependency: " + name)
        commands[-1] += ["--extern", name + "=" + str(candidates[0])]
    commands[-1] += [str(SOURCE / "native-transport-probe.rs"), "-o", str(output / "probe.a")]
    subprocess.run(commands[-1], check=True, env=environment)
    program = output / "native-transport-probe"
    commands.append([str(compiler), "-static", "-O2", "-Wall", "-Wextra", "-Werror", "-Wl,--gc-sections",
                     str(SOURCE / "native-transport-probe.c"), str(output / "probe.a"), "-ldl", "-lm", "-o", str(program)])
    subprocess.run(commands[-1], check=True)
    sources = {}
    for package in ("scarlet-abi", "scarlet-sys", "scarlet-os", "sws-protocol", "sws-client", "sws-client-c"):
        for path in (ROOT / "user/lib" / package).rglob("*"):
            if path.is_file() and (path.suffix == ".rs" or path.name == "Cargo.toml") and "target" not in path.parts:
                sources[str(path.relative_to(ROOT))] = sha256(path)
    for name in ("build-sws-client.py", "native-transport-probe.rs", "native-transport-probe.c"):
        sources[str((SOURCE / name).relative_to(ROOT))] = sha256(SOURCE / name)
    record = {"kind": "android-sws-client-v1", "target": "aarch64-linux-android", "commands": commands,
              "rustc": subprocess.check_output([str(toolchain / "rustc"), "-Vv"], text=True),
              "sources": sources, "needed": elf_needed(shared),
              "files": {path.name: sha256(path) for path in (shared, program)},
              "display_readiness": "not tested"}
    (output / "build.json").write_text(json.dumps(record, indent=2) + "\n")
    print("Built Android SWS client and SDK transport probe; no display connection tested")


if __name__ == "__main__":
    main()
