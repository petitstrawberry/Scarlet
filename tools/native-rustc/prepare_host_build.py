#!/usr/bin/env python3
"""Emit an isolated stage2 native-host bootstrap recipe; never compile or fetch.

The heavy command belongs in scarlet-rust-nix Actions. The build-host stage1
compiler uses existing LLVM; the Scarlet compiler uses dummy or Cranelift only.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

TARGETS = ("aarch64-unknown-scarlet", "riscv64gc-unknown-scarlet")
BUILD_HOSTS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin")


def executable(value):
    path = Path(value).absolute()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise argparse.ArgumentTypeError(f"not an executable file: {path}")
    # Preserve wrapper/symlink filenames: an LLVM multicall binary may dispatch
    # differently if its basename is resolved from ld.lld to lld.
    return path


def make_config(args, host):
    q = lambda value: json.dumps(str(value))
    boolean = lambda value: "true" if value else "false"
    return f'''# Generated for Rust source 39c689 and its native Scarlet patches.
change-id = "ignore"

[build]
build = {q(host)}
host = [{q(args.target)}]
target = [{q(args.target)}]
rustc = {q(args.stage0_rustc)}
cargo = {q(args.stage0_cargo)}
{("rustfmt = " + q(args.stage0_rustfmt) + chr(10)) if args.stage0_rustfmt else ""}build-dir = {q(args.build_dir)}
submodules = false
docs = false
compiler-docs = false
extended = false
full-bootstrap = false
sanitizers = false
profiler = false
vendor = {boolean(args.vendor)}
locked-deps = true
patch-binaries-for-nix = {boolean(args.nix)}

[llvm]
download-ci-llvm = false

[rust]
channel = "nightly"
download-rustc = false
# Build-host stage1 must still generate code for the native compiler.
codegen-backends = ["llvm"]
lld = false
llvm-tools = false
jemalloc = false
lto = "off"
debuginfo-level = 0
incremental = false
codegen-tests = false
deny-warnings = false

[target.{q(host)}]
llvm-config = {q(args.host_llvm_config)}
llvm-has-rust-patches = {boolean(args.host_llvm_has_rust_patches)}

[target.{q(args.target)}]
codegen-backends = [{q(args.backend)}]
optimized-compiler-builtins = false
# Bootstrap's default -Wl,... rpath arguments require a C linker driver;
# Scarlet uses direct rust-lld, and scarlet-ld does not implement RUNPATH yet.
rpath = false
# Match Scarlet's abort-only runtime and loader's 4 KiB mapping granularity.
# The interpreter supplies dl* imports of rustc_driver at runtime. This only
# permits undefined references from DSOs; undefined executable objects fail.
rustflags = ["-Cpanic=abort", "-Clink-arg=-z", "-Clink-arg=max-page-size=4096", "-Clink-arg=--allow-shlib-undefined"]
cc = {q(args.native_cc)}
cxx = {q(args.native_cxx)}
ar = {q(args.native_ar)}
linker = {q(args.native_linker)}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--rust-source", type=Path, required=True, help="Prepared, writable, isolated Rust source")
    p.add_argument("--stage0-rustc", type=executable, required=True)
    p.add_argument("--stage0-cargo", type=executable, required=True)
    p.add_argument("--stage0-rustfmt", type=executable, help="Reuse this rustfmt; otherwise infer a sibling of stage0-rustc when available")
    p.add_argument("--host-llvm-config", type=executable, required=True)
    p.add_argument("--host-llvm-has-rust-patches", action="store_true")
    p.add_argument("--native-cc", type=executable, required=True, help="Cross C compiler wrapper")
    p.add_argument("--native-cxx", type=executable, required=True, help="Cross C++ compiler wrapper")
    p.add_argument("--native-ar", type=executable, required=True)
    p.add_argument("--native-linker", type=executable, required=True, help="Direct GNU-flavor LLD for cross linking")
    p.add_argument("--target", choices=TARGETS, default=TARGETS[0])
    p.add_argument("--backend", choices=("dummy", "cranelift"), default="dummy")
    p.add_argument("--build-dir", type=Path, required=True, help="Isolated build/cache directory")
    p.add_argument("--output", type=Path, required=True, help="Fresh recipe directory")
    p.add_argument("--python", type=executable, default=Path(sys.executable).absolute())
    p.add_argument("--skip-stage0-validation", action="store_true", help="Explicitly accept a non-src/stage0 compiler version")
    p.add_argument("--vendor", action="store_true", help="Source has prepared vendor files and valid checksums/lockfile")
    p.add_argument("--offline", action="store_true", help="Set CARGO_NET_OFFLINE in the emitted build environment")
    p.add_argument("--nix", action="store_true", help="Enable bootstrap's Nix binary handling")
    args = p.parse_args()
    if args.stage0_rustfmt is None:
        sibling = args.stage0_rustc.parent / "rustfmt"
        if sibling.is_file() and os.access(sibling, os.X_OK):
            args.stage0_rustfmt = sibling.absolute()
    source = args.rust_source.resolve()
    output = args.output.resolve()
    args.build_dir = args.build_dir.resolve()
    if not (source / "x.py").is_file() or not (source / "compiler/rustc/Cargo.toml").is_file():
        p.error("rust-source must be a complete isolated Rust checkout")
    if args.build_dir == source / "build":
        p.error("use a dedicated build directory, not the checkout's default build directory")
    if output.exists():
        p.error("output already exists; preserve old evidence and choose a fresh recipe directory")
    if args.vendor and not (source / "vendor").is_dir():
        p.error("--vendor requires the prepared source's vendor directory")
    version = subprocess.check_output([str(args.stage0_rustc), "-Vv"], text=True)
    hosts = [line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: ")]
    if len(hosts) != 1 or hosts[0] not in BUILD_HOSTS:
        p.error("stage0 must run on a supported Linux or macOS build host")
    host = hosts[0]
    target_list = subprocess.check_output([str(args.stage0_rustc), "--print", "target-list"], text=True).splitlines()
    # Source 39c689 lists Scarlet among STAGE0_MISSING_TARGETS; its sanity check
    # deliberately rejects a patched stage0 containing one of those targets.
    env = {}
    if any(target.endswith("-scarlet") for target in target_list):
        env["BOOTSTRAP_SKIP_TARGET_SANITY"] = "1"
    if args.offline:
        env["CARGO_NET_OFFLINE"] = "true"
    output.mkdir(parents=True)
    config = output / "bootstrap.toml"
    config.write_text(make_config(args, host))
    command = [str(args.python), str(source / "x.py"), "build", "compiler/rustc",
               "--config", str(config), "--build-dir", str(args.build_dir),
               "--stage", "2", "--host", args.target, "--target", args.target]
    if args.skip_stage0_validation:
        command.append("--skip-stage0-validation")
    native_sysroot = args.build_dir / args.target / "stage2"
    native_lib = Path("lib/rustlib") / args.target / "lib"
    recipe = {
        "cwd": str(source), "argv": command, "env": env,
        "build_host": host, "native_target": args.target, "backend": args.backend,
        "stage0_version": version.strip(), "compiled": False,
        "native_sysroot": str(native_sysroot),
        "stdlib_source": str(args.build_dir / host / "stage1" / native_lib),
        "stdlib_destination": str(native_sysroot / native_lib),
        "packaging_note": "After build, copy matching stage1 native target libraries into the native stage2 sysroot. Do not replace them with installed cached std.",
    }
    (output / "build-command.json").write_text(json.dumps(recipe, indent=2) + "\n")
    prefix = [f"{key}={value}" for key, value in env.items()]
    print("Recipe only; execute the command in the scarlet-rust-nix Actions build.")
    print("cd " + shlex.quote(str(source)))
    print(shlex.join(["env", *prefix, *command]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
