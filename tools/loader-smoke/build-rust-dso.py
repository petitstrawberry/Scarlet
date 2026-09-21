#!/usr/bin/env python3
"""Build a real no_std Rust cdylib with an isolated experimental Scarlet target."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("aarch64", "riscv64"), default="aarch64")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--toolchain", type=Path, help="Scarlet toolchain prefix containing bin/rustc and bin/cargo")
    parser.add_argument("--offline", action="store_true", help="do not access the Cargo registry network")
    args = parser.parse_args()
    if args.toolchain:
        tool_bin = args.toolchain.resolve() / "bin"
        rustc, cargo, rustdoc = [str(tool_bin / name) for name in ("rustc", "cargo", "rustdoc")]
    else:
        rustc = shutil.which("rustc")
        cargo = shutil.which("cargo")
        rustdoc = shutil.which("rustdoc")
        if not rustc or not cargo or not rustdoc:
            parser.error("rustc/cargo/rustdoc are missing; use --toolchain or put a Scarlet toolchain on PATH")
        tool_bin = Path(rustc).parent
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parent / "rust-dso"
    original = "aarch64-unknown-scarlet" if args.arch == "aarch64" else "riscv64gc-unknown-scarlet"
    sysroot = Path(subprocess.check_output([rustc, "--print", "sysroot"], text=True).strip())
    if not (sysroot / "lib/rustlib/src/rust/library/core/Cargo.toml").is_file():
        parser.error("this toolchain needs real Rust library sources for -Zbuild-std; its sysroot is never modified")
    spec = json.loads(subprocess.check_output([rustc, "--print", "target-spec-json", "-Zunstable-options", "--target", original], text=True))
    # Keep the compiler's current native ABI and ISA. Only this generated target
    # permits shared-object output; the installed target and sysroot stay intact.
    spec["dynamic-linking"] = True
    spec["relocation-model"] = "pic"
    spec["dll-prefix"] = "lib"
    spec["dll-suffix"] = ".so"
    soname = "libsmoke-rust.so"
    spec.setdefault("pre-link-args", {}).setdefault("gnu-lld", []).extend([
        "-z", "max-page-size=4096", "-z", "now", "--hash-style=both", "-soname", soname,
    ])
    spec.setdefault("metadata", {})["description"] = f"Experimental Scarlet {args.arch} no_std Rust cdylib smoke target"
    spec["metadata"]["std"] = False
    target = output / f"scarlet-dso-{args.arch}.json"
    target.write_text(json.dumps(spec, indent=2) + "\n")
    env = os.environ.copy()
    env.update({"RUSTC": rustc, "RUSTDOC": rustdoc, "CARGO_TARGET_DIR": str(output / "cargo")})
    env["PATH"] = str(tool_bin) + os.pathsep + env.get("PATH", "")
    command = [cargo, "build", "--release", "--manifest-path", str(source / "Cargo.toml"),
               "--target", str(target), "-Zbuild-std=core,compiler_builtins"]
    if args.offline:
        command.append("--offline")
    subprocess.run(command, env=env, check=True)
    artifact = output / "cargo" / target.stem / "release/libscarlet_smoke_rust.so"
    data = bytearray(artifact.read_bytes())
    expected_machine = 183 if args.arch == "aarch64" else 243
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01" or struct.unpack_from("<HH", data, 16) != (3, expected_machine):
        raise RuntimeError("compiler output is not the expected little-endian ELF64 ET_DYN image")
    data[7] = 83  # ELFOSABI_SCARLET, matching the C smoke fixture packaging.
    destination = output / "staging/system/lib" / soname
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(data)
    manifest = {
        "rustc": subprocess.check_output([rustc, "--version", "--verbose"], text=True).strip(),
        "original_target": original,
        "experimental_target": str(target),
        "build_command": command,
        "artifact": str(destination),
        "sha256": hashlib.sha256(data).hexdigest(),
        "abi": "C entry point in a Rust cdylib; this does not claim Rust dylib/std/TLS support",
    }
    (output / "build.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(destination)


if __name__ == "__main__":
    main()
