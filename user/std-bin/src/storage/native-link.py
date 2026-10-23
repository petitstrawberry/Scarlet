#!/usr/bin/env python3
"""Cadence-style link-only ELF seed and final native import audit.

LLD 20+ needs a shared input to retain dynamic imports in a PIE. The seed has
only a null symbol and is discarded by --as-needed; it is never packaged.
Only the resident Scarlet loader's dl* APIs may remain unresolved.
"""
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys

TARGETS = {
    "aarch64-unknown-scarlet": (183, {"R_AARCH64_RELATIVE", "R_AARCH64_ABS64", "R_AARCH64_GLOB_DAT", "R_AARCH64_JUMP_SLOT"}),
    "riscv64gc-unknown-scarlet": (243, {"R_RISCV_RELATIVE", "R_RISCV_64", "R_RISCV_JUMP_SLOT"}),
}
REQUIRED = {"dlopen", "dlsym", "dlerror"}
ALLOWED = REQUIRED | {"dlclose"}


def seed(path, machine):
    data = bytearray(281)
    data[:8] = b"\x7fELF\x02\x01\x01\x53"
    for fmt, offset, value in [
        ("H", 16, 3), ("H", 18, machine), ("I", 20, 1), ("Q", 40, 64),
        ("H", 52, 64), ("H", 58, 64), ("H", 60, 3),
        ("I", 132, 11), ("Q", 136, 2), ("Q", 152, 256), ("Q", 160, 24),
        ("I", 168, 2), ("I", 172, 1), ("Q", 176, 8), ("Q", 184, 24),
        ("I", 196, 3), ("Q", 200, 2), ("Q", 216, 280), ("Q", 224, 1),
        ("Q", 240, 1),
    ]:
        struct.pack_into("<" + fmt, data, offset, value)
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists() or path.read_bytes() != data:
        path.write_bytes(data)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def imports(text):
    result = set()
    for line in text.splitlines():
        fields = line.split()
        if len(fields) >= 8 and fields[6] == "UND":
            require(fields[4:6] == ["GLOBAL", "DEFAULT"], "unexpected unresolved symbol binding")
            result.add(fields[7])
    return result


def audit(path, machine, relocations_allowed):
    data = path.read_bytes()
    require(len(data) >= 64 and data[:8] == b"\x7fELF\x02\x01\x01\x53", "not native Scarlet ELF64")
    require(struct.unpack_from("<HH", data, 16) == (3, machine), "not a PIE for the requested target")
    offset = struct.unpack_from("<Q", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 54)
    require(size == 56 and count > 0 and offset + size * count <= len(data), "invalid program headers")
    interpreters = []
    for index in range(count):
        kind, _, start, _, _, length, _, _ = struct.unpack_from("<IIQQQQQQ", data, offset + index * size)
        require(kind != 7, "native TLS is unsupported")
        if kind == 3:
            require(start + length <= len(data), "invalid interpreter")
            interpreters.append(data[start:start + length])
    require(interpreters == [b"/bin/scarlet-ld\0"], "expected exactly /bin/scarlet-ld")
    tool = shutil.which("readelf") or shutil.which("llvm-readelf")
    require(tool is not None, "readelf or llvm-readelf is required")
    def read(option):
        return subprocess.check_output([tool, "-W", option, str(path)], text=True)
    dynamic, dynsym, symbols, relocs = map(read, ["--dynamic", "--dyn-syms", "--symbols", "--relocs"])
    for tag in ["NEEDED", "RPATH", "RUNPATH", "VERNEED", "VERSYM", "VERDEF", "TEXTREL", "RELR", "RELRSZ", "RELRENT"]:
        require("(" + tag + ")" not in dynamic, "unexpected dynamic tag " + tag)
    require(re.search(r"\(FLAGS(?:_1)?\).*\b(?:BIND_NOW|NOW)\b", dynamic), "eager binding required")
    require(re.search(r"\(FLAGS_1\).*\bPIE\b", dynamic), "DF_1_PIE required")
    require("Symbol table '.symtab'" in symbols, "audit before stripping the symbol table")
    require(not re.search(r"\b(?:TLS|IFUNC)\b", symbols), "TLS/IFUNC symbols unsupported")
    found = imports(dynsym)
    require(REQUIRED <= found <= ALLOWED, "unexpected dynamic imports: " + repr(sorted(found)))
    require(imports(symbols) == found, "static/dynamic unresolved symbols differ")
    kinds = set(re.findall(r"\bR_[A-Z0-9_]+\b", relocs))
    require(kinds and kinds <= relocations_allowed, "unsupported relocation kinds: " + repr(sorted(kinds)))
    for name in found:
        require(sum(bool(re.search(r"\b" + name + r"\s*\+", line)) for line in relocs.splitlines()) == 1,
                "expected one dynamic relocation for " + name)
    print(json.dumps({"path": str(path), "imports": sorted(found), "needed_libraries": [],
                      "sha256": hashlib.sha256(data).hexdigest()}))


if __name__ == "__main__":
    try:
        require(len(sys.argv) == 4 and sys.argv[1] in {"seed", "audit"}, "expected seed|audit TARGET FILE")
        machine, relocations = TARGETS[sys.argv[2]]
        path = Path(sys.argv[3])
        if sys.argv[1] == "seed":
            seed(path, machine)
        else:
            audit(path, machine, relocations)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        sys.exit("Storage native link validation failed: " + str(error))
