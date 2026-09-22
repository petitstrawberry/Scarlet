#!/usr/bin/env python3
"""Check the Scarlet C headers for both supported target ABIs.

Uses --clang, SCARLET_PROBE_CC, or clang from PATH, in that order. Only Scarlet
headers and Clang's builtin headers are visible; a host libc cannot fill gaps.
No objects are linked or executed, so a cross-target runtime is not required.
"""

import argparse
import os
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--clang",
        default=os.environ.get("SCARLET_PROBE_CC", "clang"),
        help="Clang executable (default: SCARLET_PROBE_CC or clang)",
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    include = root / "include"
    headers = sorted(include.rglob("*.h"))
    if not headers:
        parser.error(f"no public headers found in {include}")

    # -nostdinc suppresses default host include directories. These environment
    # variables can independently inject include directories, so clear them too.
    environment = os.environ.copy()
    for variable in ("CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH"):
        environment.pop(variable, None)

    try:
        resource = subprocess.run(
            [args.clang, "-print-resource-dir"],
            check=True,
            capture_output=True,
            text=True,
            env=environment,
        ).stdout.strip()
        builtin_include = Path(resource) / "include"
        if not (builtin_include / "stddef.h").is_file():
            parser.error(f"Clang builtin stddef.h not found in {builtin_include}")

        count = 0
        for target, arch_flags in (
            ("aarch64-none-elf", []),
            ("riscv64-unknown-elf", ["-march=rv64gc", "-mabi=lp64d"]),
        ):
            for language, standard, fixture in (
                ("c", "c11", "headers.c"),
                ("c++", "c++11", "headers.cpp"),
            ):
                command = [
                    args.clang, "-target", target, *arch_flags,
                    "-ffreestanding", "-fno-builtin", "-nostdinc",
                    "-isystem", str(builtin_include), "-I", str(include),
                    "-Wall", "-Wextra", "-Werror", "-pedantic-errors",
                    f"-std={standard}", "-x", language, "-fsyntax-only",
                ]
                for header in headers:
                    name = header.relative_to(include).as_posix()
                    # Each header must include its own dependencies and tolerate
                    # repeated inclusion. The declaration avoids an empty C TU.
                    source = (
                        f"#include <{name}>\n#include <{name}>\n"
                        "extern int scarlet_header_standalone_check;\n"
                    )
                    subprocess.run(
                        [*command, "-"], input=source, text=True,
                        check=True, env=environment,
                    )
                    count += 1

                for char_flags in ([], ["-fsigned-char"], ["-funsigned-char"]):
                    subprocess.run(
                        [*command, *char_flags, str(root / "tests" / fixture)],
                        check=True, env=environment,
                    )
                    count += 1
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"Scarlet header check failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr, file=sys.stderr)
        return 1

    print(
        f"{count} header compiler checks passed "
        "(AArch64/RV64, C11/C++11, standalone/repeated includes, "
        "default/signed/unsigned char; no host libc headers)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
