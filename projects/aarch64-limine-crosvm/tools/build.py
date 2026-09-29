#!/usr/bin/env python3
"""Build pinned upstream crosvm on Linux/arm64 and prepare a Scarlet smoke root."""
import argparse
from pathlib import Path
import shutil
import subprocess

from fixture import activate_profile

PROJECT = Path(__file__).resolve().parents[1]
ROOT = PROJECT.parents[1]
SOURCE = Path(__file__).resolve().parent
REVISION = "88c1385ccb8cb31e8e4d683f580c37057cfc4b48"


def run(*args, **kwargs):
    subprocess.run(list(map(str, args)), check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=PROJECT / ".scarlet/base")
    parser.add_argument("--source", type=Path, help="existing clean checkout at the pinned revision")
    parser.add_argument("--gpu", action="store_true",
                        help="include experimental virtio-gpu 2D support (no 3D renderer)")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = args.source.resolve() if args.source else output / "upstream"
    if not source.exists():
        run("git", "init", source)
        run("git", "-C", source, "remote", "add", "origin", "https://chromium.googlesource.com/crosvm/crosvm")
        run("git", "-C", source, "fetch", "--depth=1", "origin", REVISION)
        run("git", "-C", source, "checkout", "--detach", REVISION)
        run("git", "-C", source, "submodule", "update", "--init", "--depth=1", "third_party/minijail")
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    dirty = subprocess.check_output(["git", "-C", str(source), "status", "--porcelain"], text=True)
    if actual != REVISION or dirty:
        parser.error("crosvm checkout must be clean and at " + REVISION)
    staging = output / "staging"
    linux = staging / "systems/linux-aarch64"
    for name in ["dev", "mnt/newroot", "home", "shared", "tmp", "root", "bin"]:
        (staging / name).mkdir(parents=True, exist_ok=True)
    for name in ["usr/bin", "lib", "guest", "root", "tmp", "dev", "proc", "sys"]:
        (linux / name).mkdir(parents=True, exist_ok=True)
    builder = "scarlet-crosvm-gpu-builder" if args.gpu else "scarlet-crosvm-builder"
    run("docker", "build", "--platform", "linux/arm64", "--build-arg",
        "CROSVM_GPU=" + ("1" if args.gpu else "0"), "-t", builder, SOURCE)
    export = r'''
import pathlib, re, shutil, subprocess
binary = pathlib.Path('/src/target/release/crosvm')
root = pathlib.Path('/export')
shutil.copy2(binary, root / 'usr/bin/crosvm')
for path in re.findall(r'(?:=>\s+)?(/[^\s]+)', subprocess.check_output(['ldd', str(binary)], text=True)):
    target = root / path.lstrip('/')
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path, target)
'''
    run("docker", "run", "--rm", "--platform", "linux/arm64",
        "--mount", f"type=bind,source={source},target=/src",
        "--mount", f"type=bind,source={linux},target=/export",
        builder, "sh", "-c",
        'cargo build --locked --release --no-default-features --features "$2" && python3 -c "$1"',
        "export-crosvm", export, "default-no-sandbox" + (",gpu" if args.gpu else ""))
    for manifest, target, binary in [(PROJECT / "init/Cargo.toml", "init", "crosvm-smoke-init"),
                                      (ROOT / "user/scarlet-ld/Cargo.toml", "loader", "scarlet-ld")]:
        run("cargo", "build", "--manifest-path", manifest, "--target", "aarch64-unknown-scarlet",
            "--release", "--target-dir", output / target)
        destination = staging / ("init" if target == "init" else "bin/scarlet-ld")
        shutil.copy2(output / target / "aarch64-unknown-scarlet/release" / binary, destination)
    run("clang", "--target=aarch64-unknown-none-elf", "-c", PROJECT / "fixtures/guest.S", "-o", output / "guest.o")
    run("ld.lld", "-Ttext=0x80080000", "--oformat=binary", output / "guest.o", "-o", linux / "guest/Image")
    (output / "crosvm-revision.txt").write_text(REVISION + "\n")
    activate_profile(output)
    print(staging)


if __name__ == "__main__":
    main()
