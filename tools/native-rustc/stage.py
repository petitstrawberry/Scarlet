#!/usr/bin/env python3
"""Create a fresh rootfs overlay from an already built native Scarlet rustc sysroot.

This does not build rustc or certify runtime success. It rejects the cached
macOS/Linux cross-compiler and records the exact staged files and ELF requirements.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import sys

from audit_elf import Elf, ElfError, MACHINES

TARGETS = {"riscv64gc-unknown-scarlet": "riscv64", "aarch64-unknown-scarlet": "aarch64"}
INTERPRETER = "/system/bin/scarlet-ld"


def object_report(path):
    """Audit link inputs separately: a CRT object has sections, not PT_LOADs."""
    data = path.read_bytes()
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ElfError(f"expected little-endian ELF64 object: {path}")
    kind, machine, version, _, _, shoff, _, ehsize, _, phnum, shsize, shnum, shstr = struct.unpack_from(
        "<HHIQQQIHHHHHH", data, 16)
    if kind != 1 or version != 1 or ehsize != 64 or phnum != 0:
        raise ElfError(f"expected version-1 relocatable ELF object: {path}")
    if data[7] not in (0, 0x53):
        raise ElfError(f"unexpected object OSABI: {path}")
    if shsize != 64 or not 0 < shnum < 65535 or shstr >= shnum or shoff < 64 or shoff + shnum * shsize > len(data):
        raise ElfError(f"invalid object section headers: {path}")
    for index in range(shnum):
        _, section_kind, _, _, offset, size, _, _, _, _ = struct.unpack_from(
            "<IIQQQQIIQQ", data, shoff + index * shsize)
        if section_kind != 8 and (offset > len(data) or size > len(data) - offset):
            raise ElfError(f"object section extends outside the file: {path}")
    return {"path": str(path), "sha256": hashlib.sha256(data).hexdigest(),
            "bytes": len(data), "elf_type": "REL", "machine": MACHINES.get(machine, f"unknown:{machine}"),
            "osabi": data[7]}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--sysroot", type=Path, required=True)
    p.add_argument("--target", choices=TARGETS, required=True)
    p.add_argument("--source-commit", required=True, help="Exact Rust fork revision used for every artifact")
    p.add_argument("--output", type=Path, required=True, help="New overlay directory")
    p.add_argument("--loader", type=Path, required=True, help="Built native Scarlet scarlet-ld")
    p.add_argument("--probe", type=Path, required=True, help="Cross-built native-rustc-probe")
    args = p.parse_args()
    source = args.sysroot.resolve()
    output = args.output.resolve()
    if output.exists():
        p.error("output already exists; use a fresh directory")
    if len(args.source_commit) != 40 or any(c not in "0123456789abcdef" for c in args.source_commit):
        p.error("source-commit must be a full lowercase Git SHA")
    relative = Path("opt/native-rustc")
    copies = {relative / "bin/rustc": source / "bin/rustc",
              Path("system/bin/scarlet-ld"): args.loader.resolve(),
              Path("system/bin/native-rustc-probe"): args.probe.resolve()}
    target_lib = Path("lib/rustlib") / args.target / "lib"
    if not list((source / target_lib).glob("libstd-*.rlib")):
        p.error("native sysroot is missing the matching target libstd rlib")
    for path in (source / target_lib).rglob("*"):
        if path.is_file() and path.suffix in (".rlib", ".rmeta", ".a", ".o"):
            copies[relative / path.relative_to(source)] = path
    shared = [p for p in (source / "lib").glob("*.so*") if p.is_file()]
    backend_dir = Path("lib/rustlib") / args.target / "codegen-backends"
    shared += [p for p in (source / backend_dir).glob("*.so*") if p.is_file()]
    if not any(p.name.startswith("librustc_driver-") for p in shared):
        p.error("native sysroot is missing librustc_driver-*.so")
    for path in shared:
        copies[relative / path.relative_to(source)] = path
        # scarlet-ld's first version ignores RUNPATH/RPATH. Hash-qualified Rust
        # filenames remain specific to this toolchain even in this search directory.
        runtime_path = Path("system/lib") / path.name
        if runtime_path in copies and copies[runtime_path].read_bytes() != path.read_bytes():
            p.error(f"conflicting runtime library basename: {path.name}")
        copies[runtime_path] = path
    reports = {}
    for dest, src in copies.items():
        if src.suffix in (".rlib", ".rmeta", ".a"):
            continue
        report = object_report(src) if src.suffix == ".o" else Elf(src).report()
        if report["machine"] != TARGETS[args.target]:
            p.error(f"wrong ELF machine: {src}")
        if dest in (relative / "bin/rustc", Path("system/bin/scarlet-ld"), Path("system/bin/native-rustc-probe")):
            if report["osabi"] != 0x53:
                p.error(f"expected native Scarlet OSABI 0x53: {src}")
        if dest == relative / "bin/rustc" and report["interpreter"] != INTERPRETER:
            p.error(f"rustc must request {INTERPRETER}")
        if dest == Path("system/bin/scarlet-ld") and (report["interpreter"] or report["needed"]):
            p.error("the interpreter itself must be statically linked")
        report["source"] = report.pop("path")
        reports[str(dest)] = report
    # Validate everything before creating an output overlay.
    output.mkdir(parents=True)
    manifest = {"rust_source_commit": args.source_commit, "target": args.target,
                "guest_sysroot": "/opt/native-rustc", "executed_on_scarlet": False,
                "files": [], "elf": reports}
    for dest, src in sorted(copies.items()):
        path = output / dest
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, path, follow_symlinks=True)
        manifest["files"].append({"path": str(dest), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    (output / "native-rustc-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Staged {len(copies)} files in {output}; guest probes have not run.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ElfError) as error:
        print(f"stage failed: {error}", file=sys.stderr)
        sys.exit(1)
