#!/usr/bin/env python3
"""Link and audit one plain C main using Scarlet's std-backed libc and CRT.

This checks a bounded C startup fixture, not a complete C SDK. The resulting
binary must run in Scarlet and return 43 before the startup gate is satisfied.
No Rust executable source wrapper or host C runtime is linked.
"""

import argparse
from collections import Counter
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


TARGETS = {
    "aarch64-unknown-scarlet": ("aarch64", "aarch64-none-elf", 183, []),
    "riscv64gc-unknown-scarlet": (
        "riscv64", "riscv64-unknown-elf", 243, ["-march=rv64gc", "-mabi=lp64d"],
    ),
}
REQUIRED_SYMBOLS = (
    "_start", "__scarlet_start", "before_main", "main", "__errno_location",
    "malloc", "free", "aligned_alloc", "posix_memalign", "realloc", "reallocarray",
)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def archive_members(path, expected_machine):
    """Reject a foreign or non-ELF member without depending on host ar/readelf."""
    data = path.read_bytes()
    if not data.startswith(b"!<arch>\n"):
        raise ValueError(f"expected a regular static archive: {path}")
    offset, count = 8, 0
    osabis = Counter()
    while offset < len(data):
        header = data[offset:offset + 60]
        if len(header) != 60 or header[58:60] != b"`\n":
            raise ValueError(f"invalid archive member header at {offset}")
        name = header[:16].decode("ascii").strip()
        size = int(header[48:58])
        start = offset + 60
        payload = data[start:start + size]
        if len(payload) != size:
            raise ValueError("truncated archive member")
        offset = start + size + (size & 1)
        if name in ("/", "//", "/SYM64/"):
            continue
        if name.startswith("#1/"):
            # BSD archives put the filename at the start of the member data.
            name_length = int(name[3:])
            name = payload[:name_length].rstrip(b"\0").decode("utf-8")
            payload = payload[name_length:]
            if name.startswith("__.SYMDEF"):
                continue
        if len(payload) < 64 or payload[:7] != b"\x7fELF\x02\x01\x01":
            raise ValueError(f"non-ELF64 archive member: {name}")
        machine = int.from_bytes(payload[18:20], "little")
        if machine != expected_machine or payload[7] not in (0, 0x53):
            raise ValueError(f"foreign archive member: {name}, machine={machine}, OSABI={payload[7]}")
        count += 1
        osabis[str(payload[7])] += 1
    if count == 0:
        raise ValueError("archive contains no ELF objects")
    return {"elf_members": count, "machine": expected_machine, "osabi_counts": dict(osabis)}


def audit_symbols(elf):
    """Inspect the ordinary symbol table and constructor array of this unstripped ELF."""
    section_offset = elf.unpack("Q", 40)[0]
    section_size, section_count = elf.unpack("HH", 58)
    if section_size != 64 or section_count == 0:
        raise ValueError("expected ordinary ELF64 section headers")
    sections = [elf.unpack("IIQQQQIIQQ", section_offset + i * section_size)
                for i in range(section_count)]
    defined, undefined, constructors = {}, [], []
    for section in sections:
        kind, offset, size, link, entry_size = section[1], section[4], section[5], section[6], section[9]
        if kind == 14:  # SHT_INIT_ARRAY
            if size % 8:
                raise ValueError("partial constructor array entry")
            constructors.extend(elf.unpack("Q", offset + i)[0] for i in range(0, size, 8))
        if kind != 2:  # SHT_SYMTAB
            continue
        if entry_size != 24 or size % 24 or link >= len(sections):
            raise ValueError("invalid ELF symbol table")
        strings = sections[link]
        names = elf.slice(strings[4], strings[5])
        for position in range(offset, offset + size, 24):
            name, _, _, index, value, _ = elf.unpack("IBBHQQ", position)
            if not name:
                continue
            name = elf.cstring(names, name)
            if index == 0:
                undefined.append(name)
            else:
                defined[name] = value
    missing = sorted(set(REQUIRED_SYMBOLS) - defined.keys())
    if missing or undefined:
        raise ValueError(f"missing symbols: {missing}; undefined symbols: {undefined}")
    if elf.entry != defined["_start"] or defined["before_main"] not in constructors:
        raise ValueError("ELF entry or C constructor does not match the fixture")
    return {
        "required_defined_symbols": {name: defined[name] for name in REQUIRED_SYMBOLS},
        "undefined_symbols": undefined,
        "constructor_addresses": constructors,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--sysroot", required=True, type=Path, help="matching Scarlet rustc sysroot")
    parser.add_argument("--libc", required=True, type=Path, help="fresh libscarlet_c.a, including std")
    parser.add_argument("--output", required=True, type=Path, help="new output directory (must not exist)")
    parser.add_argument("--clang", default=os.environ.get("SCARLET_PROBE_CC", "clang"))
    parser.add_argument("--linker", default="ld.lld")
    args = parser.parse_args()
    libc = Path(__file__).resolve().parent.parent
    repo = libc.parents[2]
    source = libc / "tests/c_main.c"
    crt = args.sysroot.resolve() / "lib/rustlib" / args.target / "lib/scarlet-crt0.o"
    archive = args.libc.resolve()
    requested_output = args.output.absolute()
    if requested_output.exists() or requested_output.is_symlink():
        parser.error(f"output directory already exists: {requested_output}")
    output = requested_output.resolve()
    for path in (crt, archive, source):
        if not path.is_file():
            parser.error(f"required input is missing: {path}")
    clang = shutil.which(args.clang)
    linker = shutil.which(args.linker)
    if clang is None or linker is None:
        parser.error("Clang and ld.lld must be executable paths or available on PATH")

    # Also suppress environment include paths: -nostdinc alone does not do so.
    environment = os.environ.copy()
    for variable in ("CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH"):
        environment.pop(variable, None)
    machine_name, triple, machine, arch_flags = TARGETS[args.target]
    try:
        output.mkdir(parents=True)
    except OSError as error:
        parser.error(f"cannot create output directory: {error}")
    commands = []

    def run(command, log):
        command = [str(item) for item in command]
        commands.append(command)
        (output / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, env=environment)
        (output / log).write_text(result.stdout)
        if result.returncode:
            raise RuntimeError(f"command failed ({result.returncode}); see {output / log}")
        return result.stdout

    try:
        member_report = archive_members(archive, machine)
        resource = Path(run([clang, "-print-resource-dir"], "clang-resource.log").strip())
        if not (resource / "include/stddef.h").is_file():
            raise ValueError(f"Clang builtin stddef.h missing in {resource}")
        obj, binary = output / "c_main.o", output / "c-startup-probe"
        run([clang, "-target", triple, *arch_flags, "-std=c11", "-ffreestanding",
             "-fno-builtin", "-fno-stack-protector", "-nostdinc",
             "-isystem", resource / "include", "-I", libc / "include",
             "-O2", "-Wall", "-Wextra", "-Werror", "-c", source, "-o", obj], "c-build.log")
        link_flags = ["--fix-cortex-a53-843419"] if machine_name == "aarch64" else []
        run([linker, "--gc-sections", "--no-undefined", "-static", "-e", "_start",
             "-z", "max-page-size=4096", *link_flags, "-Map=" + str(output / "link.map"),
             "--trace", crt, obj, archive, "-o", binary], "link.log")
        audit_path = repo / "tools/native-rustc/audit_elf.py"
        elf_report = json.loads(run([sys.executable, audit_path, "--machine", machine_name,
                                    "--scarlet", binary], "elf-audit.json"))[0]
        if (elf_report["elf_type"] != "EXEC" or elf_report["interpreter"] is not None
                or elf_report["needed"] or elf_report["tls"]
                or elf_report["undefined_relocated_symbols"]):
            raise ValueError("expected a static native executable without ELF TLS or unresolved imports")
        spec = importlib.util.spec_from_file_location("scarlet_c_startup_elf_audit", audit_path)
        audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(audit)
        symbols = audit_symbols(audit.Elf(binary))
        inputs = {name: {"path": str(path), "sha256": sha256(path)} for name, path in (
            ("source", source), ("crt", crt), ("libc", archive), ("object", obj),
        )}
        report = {
            "scope": "One plain C main and C constructor linked directly with Scarlet CRT and std-backed libc; not a complete C SDK.",
            "target": args.target,
            "expected_exit_code": 43,
            "guest_execution": "pending",
            "executed_on_scarlet": False,
            "inputs": inputs,
            "commands": commands,
            "archive": member_report,
            "elf": elf_report,
            "symbols": symbols,
        }
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    except (OSError, ValueError, RuntimeError) as error:
        print(f"Scarlet C startup check failed: {error}", file=sys.stderr)
        return 1
    print(f"Built and audited {binary}; run in Scarlet and require exit code 43.")
    print(f"Evidence: {output / 'result.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
