#!/usr/bin/env python3
"""Build unmodified, pinned zlib and a plain C Scarlet acceptance executable.

The output directory must be new. This compiles and audits only; acceptance
requires running zlib-probe in Scarlet, with a writable directory argument.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import urllib.request


VERSION = "1.3.2"
SOURCE_URL = "https://zlib.net/fossils/zlib-1.3.2.tar.gz"
SOURCE_SHA256 = "bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16"
SOURCES = (
    "adler32.c", "compress.c", "crc32.c", "deflate.c", "gzclose.c",
    "gzlib.c", "gzread.c", "gzwrite.c", "infback.c", "inffast.c",
    "inflate.c", "inftrees.c", "trees.c", "uncompr.c", "zutil.c",
)
TARGETS = {
    "aarch64-unknown-scarlet": ("aarch64", "aarch64-none-elf", 183,
                                 ["-mno-outline-atomics"]),
    "riscv64gc-unknown-scarlet": ("riscv64", "riscv64-unknown-elf", 243,
                                   ["-march=rv64gc", "-mabi=lp64d"]),
}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def unpack_source(archive, output):
    if sha256(archive) != SOURCE_SHA256:
        raise ValueError(f"source archive does not match pinned zlib {VERSION} SHA256")
    # Only regular files and directories from the verified release are needed.
    # Do not extract links, devices, or paths that could leave the output tree.
    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        for member in members:
            path = PurePosixPath(member.name)
            if (path.is_absolute() or ".." in path.parts or not path.parts
                    or path.parts[0] != f"zlib-{VERSION}"
                    or not (member.isfile() or member.isdir())):
                raise ValueError(f"unsupported archive entry: {member.name}")
        for member in members:
            path = output.joinpath(*PurePosixPath(member.name).parts)
            if member.isdir():
                path.mkdir(parents=True, exist_ok=True)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as data, path.open("xb") as target:
                    shutil.copyfileobj(data, target)
    return output / f"zlib-{VERSION}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--sysroot", required=True, type=Path)
    parser.add_argument("--libc", required=True, type=Path,
                        help="fresh matching std-backed libscarlet_c.a")
    parser.add_argument("--output", required=True, type=Path,
                        help="new output directory")
    parser.add_argument("--source-archive", type=Path,
                        help="official pinned tar.gz; otherwise download the pinned URL")
    parser.add_argument("--clang", default=os.environ.get("SCARLET_PROBE_CC", "clang"))
    parser.add_argument("--ar", default=os.environ.get("SCARLET_PROBE_AR", "llvm-ar"))
    parser.add_argument("--linker", default="ld.lld")
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    repo = here.parents[2]
    archive = args.libc.resolve()
    crt = args.sysroot.resolve() / "lib/rustlib" / args.target / "lib/scarlet-crt0.o"
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        parser.error(f"output directory already exists: {output}")
    output = output.resolve()
    for path in (archive, crt, here / "main.c"):
        if not path.is_file():
            parser.error(f"missing input: {path}")
    clang, ar, linker = (shutil.which(value) for value in (args.clang, args.ar, args.linker))
    if None in (clang, ar, linker):
        parser.error("Clang, llvm-ar, and ld.lld must be executable paths or available on PATH")
    output.mkdir(parents=True)
    environment = os.environ.copy()
    for variable in ("CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH"):
        environment.pop(variable, None)
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
        source_archive = output / f"zlib-{VERSION}.tar.gz"
        if args.source_archive is not None:
            shutil.copyfile(args.source_archive, source_archive)
        else:
            with urllib.request.urlopen(SOURCE_URL, timeout=30) as response:
                with source_archive.open("xb") as target:
                    shutil.copyfileobj(response, target)
        source = unpack_source(source_archive, output)
        if f'#define ZLIB_VERSION "{VERSION}"' not in (source / "zlib.h").read_text():
            raise ValueError("zlib header version differs from the pinned version")
        shutil.copy2(source / "LICENSE", output / "ZLIB-LICENSE")
        # Snapshot in-repository inputs so concurrent header edits cannot change
        # the meaning of the saved compiler commands or source hashes.
        includes = output / "scarlet-include"
        shutil.copytree(repo / "user/lib/scarlet-libc/include", includes)
        fixture = output / "main.c"
        shutil.copy2(here / "main.c", fixture)
        archive_hash, crt_hash = sha256(archive), sha256(crt)
        helper = load_module("scarlet_c_archive_audit",
                             repo / "user/lib/scarlet-libc/tests/build_c_startup.py")
        machine_name, triple, machine, arch_flags = TARGETS[args.target]
        libc_members = helper.archive_members(archive, machine)
        versions = {name: run([tool, "--version"], name + "-version.log").strip()
                    for name, tool in (("clang", clang), ("ar", ar), ("linker", linker))}
        resource = Path(run([clang, "-print-resource-dir"], "clang-resource.log").strip())
        if not (resource / "include/stddef.h").is_file():
            raise ValueError(f"Clang builtin headers are missing: {resource}")
        flags = ["-target", triple, *arch_flags, "-std=c11", "-ffreestanding",
                 "-fno-builtin", "-fno-stack-protector", "-ffunction-sections",
                 "-fdata-sections", "-nostdinc", "-isystem", resource / "include",
                 "-I", includes, "-I", source, "-O2", "-Wall", "-Wextra",
                 "-Werror", "-DZ_HAVE_UNISTD_H", "-DHAVE_HIDDEN"]
        objects = []
        for name in SOURCES:
            obj = output / (Path(name).stem + ".o")
            run([clang, *flags, "-c", source / name, "-o", obj], name + ".log")
            objects.append(obj)
        library = output / "libz.a"
        run([ar, "rcsD", library, *objects], "ar.log")
        zlib_members = helper.archive_members(library, machine)
        fixture_obj, binary = output / "main.o", output / "zlib-probe"
        run([clang, *flags, "-c", fixture, "-o", fixture_obj], "fixture.log")
        link_flags = ["--fix-cortex-a53-843419"] if machine_name == "aarch64" else []
        run([linker, "--gc-sections", "--no-undefined", "-static", "-e", "_start",
             "-z", "max-page-size=4096", *link_flags, "-Map=" + str(output / "link.map"),
             "--trace", crt, fixture_obj, library, archive, "-o", binary], "link.log")
        audit_path = repo / "tools/native-rustc/audit_elf.py"
        elf_report = json.loads(run([sys.executable, audit_path, "--machine", machine_name,
                                    "--scarlet", binary], "elf-audit.json"))[0]
        if (elf_report["elf_type"] != "EXEC" or elf_report["interpreter"] is not None
                or elf_report["needed"] or elf_report["tls"]
                or elf_report["undefined_relocated_symbols"]):
            raise ValueError("expected a static Scarlet executable without unresolved imports or ELF TLS")
        if sha256(archive) != archive_hash or sha256(crt) != crt_hash:
            raise ValueError("libc or CRT changed during the build; rebuild from stable inputs")
        report = {
            "scope": "Unmodified upstream zlib core and gzip fd I/O linked to Scarlet's std-backed libc; not full POSIX or C conformance.",
            "target": args.target,
            "upstream": {"name": "zlib", "version": VERSION, "url": SOURCE_URL,
                         "archive_sha256": SOURCE_SHA256, "source_modified": False},
            "expected_exit_code": 47,
            "expected_stdout_line": "SCARLET_LIBC_ZLIB_OK",
            "guest_execution": "pending",
            "executed_on_scarlet": False,
            "binary": {"path": str(binary), "sha256": sha256(binary)},
            "inputs": {"libc": {"path": str(archive), "sha256": archive_hash},
                       "crt": {"path": str(crt), "sha256": crt_hash},
                       "fixture": {"path": str(fixture), "sha256": sha256(fixture)}},
            "header_sha256": {str(path.relative_to(includes)): sha256(path)
                              for path in sorted(includes.rglob("*.h"))},
            "upstream_source_sha256": {name: sha256(source / name) for name in SOURCES},
            "commands": commands,
            "tool_versions": versions,
            "archives": {"libc": libc_members, "zlib": zlib_members},
            "elf": elf_report,
        }
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    except (OSError, ValueError, RuntimeError, tarfile.TarError) as error:
        print(f"zlib consumer build failed: {error}", file=sys.stderr)
        return 1
    print(f"Built and audited {binary}; guest execution remains required.")
    print(f"Run with one writable directory argument; require exit 47 and SCARLET_LIBC_ZLIB_OK.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
