#!/usr/bin/env python3
"""Inspect ELF64 loader requirements without executing the input or requiring readelf.

This is an inventory, not proof of Scarlet ABI compatibility or loadability.
Only file-backed program headers are used; section headers may be stripped.
"""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct
import sys

MACHINES = {183: "aarch64", 243: "riscv64"}
RELOCS = {
    183: {0: "NONE", 257: "ABS64", 258: "ABS32", 260: "PREL64", 261: "PREL32",
          1024: "COPY", 1025: "GLOB_DAT", 1026: "JUMP_SLOT", 1027: "RELATIVE",
          1028: "TLS_DTPMOD64", 1029: "TLS_DTPREL64", 1030: "TLS_TPREL64",
          1031: "TLSDESC", 1032: "IRELATIVE"},
    243: {0: "NONE", 1: "32", 2: "64", 3: "RELATIVE", 4: "COPY",
          5: "JUMP_SLOT", 6: "TLS_DTPMOD32", 7: "TLS_DTPMOD64",
          8: "TLS_DTPREL32", 9: "TLS_DTPREL64", 10: "TLS_TPREL32",
          11: "TLS_TPREL64", 12: "TLSDESC", 58: "IRELATIVE", 62: "TLSDESC_HI20"},
}


class ElfError(ValueError):
    pass


class Elf:
    def __init__(self, path):
        self.path = Path(path)
        self.data = self.path.read_bytes()
        if self.data[:7] != b"\x7fELF\x02\x01\x01":
            raise ElfError("expected little-endian ELF64 version 1 (not a host Mach-O or ELF32)")
        header = self.unpack("HHIQQQIHHHHHH", 16)
        self.kind, self.machine, version, self.entry, phoff = header[:5]
        ehsize, phentsize, phnum = header[7:10]
        if version != 1 or ehsize < 64 or self.kind not in (2, 3):
            raise ElfError("expected a version-1 ELF executable or shared object")
        if phentsize != 56 or phnum == 65535:
            raise ElfError("unsupported program-header size or extended header count")
        self.headers = []
        for index in range(phnum):
            h = self.unpack("IIQQQQQQ", phoff + index * phentsize)
            p = dict(zip(("type", "flags", "offset", "vaddr", "paddr", "filesz", "memsz", "align"), h))
            self.slice(p["offset"], p["filesz"])
            if p["type"] == 1 and p["filesz"] > p["memsz"]:
                raise ElfError("PT_LOAD file size exceeds memory size")
            self.headers.append(p)
        self.dynamic = {}
        dynamics = [p for p in self.headers if p["type"] == 2]
        if len(dynamics) > 1:
            raise ElfError("multiple PT_DYNAMIC segments")
        for p in dynamics:
            if p["filesz"] % 16:
                raise ElfError("partial dynamic entry")
            for offset in range(p["offset"], p["offset"] + p["filesz"], 16):
                tag, value = self.unpack("qQ", offset)
                if tag == 0:
                    break
                self.dynamic.setdefault(tag, []).append(value)
            else:
                raise ElfError("PT_DYNAMIC has no DT_NULL terminator")

    def slice(self, offset, size):
        if offset < 0 or size < 0 or offset > len(self.data) or size > len(self.data) - offset:
            raise ElfError("ELF data extends outside the file")
        return self.data[offset:offset + size]

    def unpack(self, fmt, offset):
        fmt = "<" + fmt
        return struct.unpack(fmt, self.slice(offset, struct.calcsize(fmt)))

    def at_vaddr(self, address, size):
        for p in self.headers:
            delta = address - p["vaddr"]
            if p["type"] == 1 and 0 <= delta <= p["filesz"] and size <= p["filesz"] - delta:
                return p["offset"] + delta
        raise ElfError(f"virtual address {address:#x} is not file-backed")

    def tag(self, tag, default=None):
        values = self.dynamic.get(tag, [])
        if len(values) > 1:
            raise ElfError(f"duplicate scalar dynamic tag {tag:#x}")
        return values[0] if values else default

    @staticmethod
    def cstring(data, offset=0):
        if not 0 <= offset < len(data):
            raise ElfError("string offset outside string table")
        end = data.find(b"\0", offset)
        if end < 0:
            raise ElfError("unterminated ELF string")
        return data[offset:end].decode("utf-8", errors="backslashreplace")

    def dynstring(self, offset):
        addr, size = self.tag(5), self.tag(10)
        if addr is None or size is None:
            raise ElfError("dynamic string lacks DT_STRTAB/DT_STRSZ")
        return self.cstring(self.slice(self.at_vaddr(addr, size), size), offset)

    def relocations(self, addr, size, kind, entsize):
        expected = 24 if kind == 7 else 16
        if entsize != expected or size % expected:
            raise ElfError("invalid relocation entry size")
        start = self.at_vaddr(addr, size)
        for offset in range(start, start + size, expected):
            _, info = self.unpack("QQ", offset)
            yield info & 0xffffffff, info >> 32

    def report(self):
        report = {
            "path": str(self.path), "sha256": hashlib.sha256(self.data).hexdigest(),
            "bytes": len(self.data), "elf_type": {2: "EXEC", 3: "DYN"}[self.kind],
            "machine": MACHINES.get(self.machine, f"unknown:{self.machine}"),
            "osabi": self.data[7], "entry": self.entry,
            "interpreter": None, "needed": [], "rpath": None, "runpath": None,
            "soname": None, "tls": [], "relocations": {}, "undefined_relocated_symbols": [],
        }
        interpreters = [p for p in self.headers if p["type"] == 3]
        if len(interpreters) > 1:
            raise ElfError("multiple PT_INTERP segments")
        if interpreters:
            p = interpreters[0]
            report["interpreter"] = self.cstring(self.slice(p["offset"], p["filesz"]))
        report["tls"] = [{k: p[k] for k in ("filesz", "memsz", "align")}
                         for p in self.headers if p["type"] == 7]
        for field, tag in (("soname", 14), ("rpath", 15), ("runpath", 29)):
            value = self.tag(tag)
            if value is not None:
                report[field] = self.dynstring(value)
        report["needed"] = [self.dynstring(v) for v in self.dynamic.get(1, [])]
        report["hash_tables"] = [name for name, tag in (("sysv", 4), ("gnu", 0x6ffffef5))
                                 if tag in self.dynamic]
        report["textrel"] = 22 in self.dynamic or bool(self.tag(30, 0) & 4)
        report["relr"] = 36 in self.dynamic or 35 in self.dynamic
        report["symbol_versioning"] = any(t in self.dynamic for t in (0x6ffffff0, 0x6ffffffc, 0x6ffffffe))
        report["init_fini"] = [name for name, tag in (("init", 12), ("fini", 13),
                               ("init_array", 25), ("fini_array", 26), ("preinit_array", 32))
                               if tag in self.dynamic]
        counts, symbol_ids, seen = Counter(), set(), set()
        tables = [(self.tag(7), self.tag(8, 0), 7, self.tag(9, 24)),
                  (self.tag(17), self.tag(18, 0), 17, self.tag(19, 16))]
        if 23 in self.dynamic:
            kind = self.tag(20)
            if kind not in (7, 17):
                raise ElfError("DT_PLTREL must identify REL or RELA")
            tables.append((self.tag(23), self.tag(2, 0), kind, 24 if kind == 7 else 16))
        for addr, size, kind, entsize in tables:
            if addr is None:
                if size:
                    raise ElfError("relocation size without relocation address")
                continue
            table = (addr, size, kind)
            if table in seen:
                continue
            seen.add(table)
            for reloc, symbol in self.relocations(addr, size, kind, entsize):
                name = RELOCS.get(self.machine, {}).get(reloc, f"UNKNOWN_{reloc}")
                counts[name] += 1
                if symbol:
                    symbol_ids.add(symbol)
        report["relocations"] = dict(sorted(counts.items()))
        symtab = self.tag(6)
        if symbol_ids:
            if symtab is None or self.tag(11, 24) != 24:
                raise ElfError("missing or unsupported dynamic symbol table")
            for index in sorted(symbol_ids):
                name, info, _, section, _, _ = self.unpack("IBBHQQ", self.at_vaddr(symtab + index * 24, 24))
                if section == 0:
                    report["undefined_relocated_symbols"].append({"name": self.dynstring(name),
                        "binding": {1: "GLOBAL", 2: "WEAK"}.get(info >> 4, str(info >> 4)),
                        "type": {6: "TLS", 10: "GNU_IFUNC"}.get(info & 15, str(info & 15))})
        return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="+", type=Path)
    parser.add_argument("--machine", choices=MACHINES.values())
    parser.add_argument("--scarlet", action="store_true", help="Require Scarlet OSABI 0x53")
    parser.add_argument("--require-interpreter", help="Require an exact PT_INTERP path on every input")
    args = parser.parse_args()
    reports, failed = [], False
    for path in args.files:
        try:
            report = Elf(path).report()
            errors = []
            if args.machine and report["machine"] != args.machine:
                errors.append(f"expected machine {args.machine}")
            if args.scarlet and report["osabi"] != 0x53:
                errors.append("expected Scarlet OSABI 0x53")
            if args.require_interpreter and report["interpreter"] != args.require_interpreter:
                errors.append(f"expected interpreter {args.require_interpreter}")
            if errors:
                report["errors"] = errors
                failed = True
            reports.append(report)
        except (OSError, ElfError) as err:
            reports.append({"path": str(path), "error": str(err)})
            failed = True
    print(json.dumps(reports, indent=2))
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
