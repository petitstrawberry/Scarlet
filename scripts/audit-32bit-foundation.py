#!/usr/bin/env python3
"""Inventory width-sensitive source for the 32-bit foundation audit.

This is a lexical review aid, not a Rust parser or a portability check. Matches
include intentional fixed-width formats, existing architecture implementations,
tests, comments, and vendored code. No match is automatically a defect, and an
absence of matches is not evidence of portability. Reads tracked working-tree
files; does not build, modify sources, or download dependencies.
"""

import argparse
import json
import re
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SUFFIXES = {
    ".rs", ".c", ".h", ".s", ".S", ".ld", ".lds", ".toml", ".json",
    ".nix", ".sh", ".py", ".yml", ".yaml",
}
RULES = {
    "native_word": (
        r"\b(?:usize|isize)\b",
        "Host-sized values: distinguish pointers/counts from persistent quantities and ABI words.",
    ),
    "wide_integer": (
        r"\b(?:u64|i64|u128|i128)\b",
        "Fixed-width values: review transport and conversions; do not narrow mechanically.",
    ),
    "native_cast": (
        r"\bas\s+(?:usize|isize)\b",
        "Potential narrowing or sign conversion; source type and range require manual review.",
    ),
    "atomic": (
        r"\bAtomic(?:Bool|Ptr|[UI](?:8|16|32|64|128|size))\b",
        "Atomic availability, ordering, and read-modify-write capability dependencies.",
    ),
    "atomic64": (
        r"\bAtomic[UI]64\b",
        "64-bit atomic dependencies, including diagnostics and test-only uses.",
    ),
    "refcount": (
        r"\b(?:Arc|Weak)\b",
        "Shared ownership dependencies; alloc::sync requires pointer atomics.",
    ),
    "layout": (
        r"repr\s*\(\s*(?:C|packed|align)|\b(?:size_of|align_of|offset_of)\s*[!:<]",
        "C/packed layouts and explicit size/alignment/offset contracts.",
    ),
    "raw_memory": (
        r"\b(?:transmute|from_raw_parts(?:_mut)?|read_unaligned|write_unaligned|"
        r"read_volatile|write_volatile)\b|\bas\s+\*(?:const|mut)\b",
        "Typed memory, byte-buffer casts, and MMIO requiring alignment/access-width review.",
    ),
    "word_bytes": (
        r"(?:u|i)size::(?:from|to)_(?:ne|le|be)_bytes|"
        r"\[\s*0(?:_?u8)?\s*;\s*(?:8|16|24|48)\s*\]|"
        r"\b(?:sp|current_pos|current_addr)\s*[-+]="
        r"\s*(?:8|16)\b",
        "Word serialization and fixed byte steps; includes valid fixed-width records.",
    ),
    "wide_shift": (
        r"(?:<<|>>)\s*(?:3[2-9]|[4-9][0-9]|1[01][0-9]|12[0-7])\b",
        "Literal shift >= 32; a u64/u128 operand can be entirely correct.",
    ),
    "long_hex": (
        r"\b0[xX][0-9a-fA-F_]{9,}",
        "Long hexadecimal spelling; underscores mean this is not a value-range test.",
    ),
    "elf64": (
        r"ELFCLASS64|Elf64|ELF64|elf64|ELF class.*64",
        "64-bit executable/object format dependencies.",
    ),
    "arch_selection": (
        r"target_(?:arch|pointer_width|has_atomic|feature)|"
        r"crate::arch::(?:riscv64|aarch64)",
        "Architecture and capability selection; includes already isolated implementations.",
    ),
    "assembly": (
        r"\b(?:asm|naked_asm|global_asm)!|\b(?:ld|sd|lwu|lr\.d|sc\.d)\s+",
        "Assembly sites and RV64 word accesses; inspect surrounding cfg and register layout.",
    ),
    "target_build": (
        r"riscv64|aarch64|lp64|target-pointer-width|max-atomic-width|data-layout",
        "Build/target/dependency references, and documentation embedded in source.",
    ),
}


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def inventory(categories):
    patterns = {name: re.compile(RULES[name][0]) for name in categories}
    tracked = sorted(git("ls-files", "-z").decode().split("\0"))
    scanned = []
    skipped = []
    findings = []
    for name in tracked:
        path = ROOT / name
        if not name or path.suffix not in SUFFIXES or path == Path(__file__).resolve():
            continue
        if path.is_symlink():
            skipped.append({"path": name, "reason": "symlink"})
            continue
        try:
            source = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            skipped.append({"path": name, "reason": type(error).__name__})
            continue
        if "\0" in source:
            skipped.append({"path": name, "reason": "binary"})
            continue
        scanned.append(name)
        matches = {}
        for number, line in enumerate(source.splitlines(), 1):
            for category, pattern in patterns.items():
                if pattern.search(line):
                    matches.setdefault(category, []).append(number)
        if matches:
            findings.append({"path": name, "matches": matches})
    return {
        "schema_version": 1,
        "source_revision": git("rev-parse", "HEAD").decode().strip(),
        "scope": "Tracked working-tree source/configuration; matches include comments and tests.",
        "limitation": "Lexical candidates only; no type, cfg reachability, or external dependency analysis.",
        "scanned_files": scanned,
        "skipped_files": skipped,
        "rules": {
            name: {
                "pattern": RULES[name][0],
                "review": RULES[name][1],
                "files": sum(name in entry["matches"] for entry in findings),
                "lines": sum(len(entry["matches"].get(name, [])) for entry in findings),
            }
            for name in categories
        },
        "findings": findings,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("summary", "json"), default="summary")
    parser.add_argument("--category", action="append", choices=tuple(RULES))
    args = parser.parse_args()
    report = inventory(list(dict.fromkeys(args.category or RULES)))
    if args.format == "json":
        print(json.dumps(report, indent=2))
        return
    print("32-bit foundation review candidates (not a pass/fail check)")
    print("Source revision:", report["source_revision"])
    print(f"Scanned: {len(report['scanned_files'])} files; "
          f"matched: {len(report['findings'])}; skipped: {len(report['skipped_files'])}")
    print(f"{'Category':<18} {'Files':>6} {'Lines':>7}")
    for name, rule in report["rules"].items():
        print(f"{name:<18} {rule['files']:>6} {rule['lines']:>7}")
    for skipped in report["skipped_files"]:
        print(f"Skipped {skipped['path']}: {skipped['reason']}")
    print("Use --format json for every matched file and source line number.")


if __name__ == "__main__":
    main()
