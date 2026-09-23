#!/usr/bin/env python3
"""Build minimal native ELF DSOs and a PIE with clang + LLD, without libc."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("aarch64", "riscv64"), default="aarch64")
    parser.add_argument("--hash-style", choices=("both", "gnu", "sysv"), default="both")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--clang", default=os.environ.get("TARGET_CC", "clang"))
    parser.add_argument("--linker", default="ld.lld")
    parser.add_argument("--loader", type=Path, help="copy built scarlet-ld into staging")
    parser.add_argument("--rust-library", type=Path, help="also test this Rust cdylib")
    args = parser.parse_args()
    source = Path(__file__).resolve().parent / "fixtures"
    output = args.output.resolve()
    objects = output / "objects"
    staging = output / "staging"
    libraries = staging / "system/lib"
    objects.mkdir(parents=True, exist_ok=True)
    libraries.mkdir(parents=True, exist_ok=True)
    target = "aarch64-unknown-none-elf" if args.arch == "aarch64" else "riscv64-unknown-none-elf"
    for name in ("dependency", "answer", "plugin", "main"):
        command = [args.clang, f"--target={target}", "-fPIC", "-ffreestanding", "-fno-stack-protector", "-fno-builtin", "-fno-asynchronous-unwind-tables", "-O1"]
        if args.rust_library:
            command.append("-DSCARLET_SMOKE_RUST_DSO=1")
        if args.arch == "riscv64":
            command += ["-march=rv64gc", "-mabi=lp64d"]
        subprocess.run(command + ["-c", str(source / f"{name}.c"), "-o", str(objects / f"{name}.o")], check=True)
    common = [args.linker, f"--hash-style={args.hash_style}", "-z", "now", "-z", "max-page-size=4096", "-L", str(libraries)]
    for name, dependencies in (("dependency", []), ("answer", ["dependency"]), ("plugin", ["answer"])):
        soname = f"libsmoke-{name}.so"
        subprocess.run(common + ["-shared", "-soname", soname, str(objects / f"{name}.o"), *[f"-lsmoke-{dep}" for dep in dependencies], "-o", str(libraries / soname)], check=True)
    # The four C loader APIs are supplied by the interpreter, not by a libc.
    # LLD records their undefined dynamic symbols and eager PLT relocations.
    subprocess.run(common + ["-pie", "--export-dynamic", "--unresolved-symbols=ignore-all", "--dynamic-linker=/bin/scarlet-ld", "-e", "_start", str(objects / "main.o"), "-lsmoke-answer", "-o", str(staging / "init")], check=True)
    for elf in [staging / "init", *libraries.glob("*.so")]:
        with elf.open("r+b") as stream:
            stream.seek(7)
            stream.write(bytes([83]))  # ELFOSABI_SCARLET
    if args.rust_library:
        shutil.copy2(args.rust_library, libraries / "libsmoke-rust.so")
    if args.loader:
        (staging / "bin").mkdir(parents=True, exist_ok=True)
        shutil.copy2(args.loader, staging / "bin/scarlet-ld")
    print(staging)


if __name__ == "__main__":
    main()
