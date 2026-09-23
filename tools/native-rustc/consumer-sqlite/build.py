#!/usr/bin/env python3
"""Build unmodified, pinned SQLite and a plain C Scarlet acceptance executable.

The output directory must be new. This compiles and audits only; acceptance
requires running sqlite-probe in Scarlet, with a writable directory argument.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import stat
import sys
import zipfile
import urllib.request


VERSION = "3.53.4"
RELEASE = "3530400"
SOURCE_URL = "https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip"
SOURCE_SHA256 = "1e71ddf93849c6a6ecf58b827c0692073d2dd7ee40196158068f7b29f422e87d"
SOURCE_SHA3_256 = "628a44cfe82c66aed1ccbbe85a562d2e33ebe64b3288981ed76285612227934e"
SOURCE_ID = "2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc"
SOURCES = ("sqlite3.c",)
CONFIGURATION = (
    "NDEBUG", "SQLITE_OS_OTHER=1", "SQLITE_THREADSAFE=1",
    "SQLITE_OMIT_LOAD_EXTENSION", "SQLITE_OMIT_LOCALTIME", "SQLITE_TEMP_STORE=3",
    "SQLITE_OMIT_WAL", "SQLITE_MAX_MMAP_SIZE=0",
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
    data = archive.read_bytes()
    if (hashlib.sha256(data).hexdigest() != SOURCE_SHA256
            or hashlib.sha3_256(data).hexdigest() != SOURCE_SHA3_256):
        raise ValueError(f"source archive does not match pinned SQLite {VERSION}")
    root = f"sqlite-amalgamation-{RELEASE}"
    expected = {root + "/", *(root + "/" + name for name in
                ("sqlite3.c", "sqlite3.h", "sqlite3ext.h", "shell.c"))}
    # The verified official ZIP contains exactly one directory and four files.
    # Do not extract links, devices, duplicate paths, or additional entries.
    with zipfile.ZipFile(archive) as source:
        entries = source.infolist()
        if len(entries) != len(expected) or {entry.filename for entry in entries} != expected:
            raise ValueError("unexpected archive member set")
        for entry in entries:
            path = PurePosixPath(entry.filename)
            mode = entry.external_attr >> 16
            if (path.is_absolute() or ".." in path.parts
                    or (stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR))):
                raise ValueError(f"unsupported archive entry: {entry.filename}")
            destination = output.joinpath(*path.parts)
            if entry.is_dir():
                destination.mkdir(parents=True, exist_ok=False)
            else:
                destination.parent.mkdir(parents=True, exist_ok=True)
                with source.open(entry) as data, destination.open("xb") as target:
                    shutil.copyfileobj(data, target)
    return output / root


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--sysroot", required=True, type=Path)
    parser.add_argument("--libc", required=True, type=Path,
                        help="fresh matching std-backed libscarlet_c.a")
    parser.add_argument("--output", required=True, type=Path,
                        help="new output directory")
    parser.add_argument("--source-archive", type=Path,
                        help="official pinned ZIP; otherwise download the pinned URL")
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
    for path in (archive, crt, here / "main.c", here / "scarlet_vfs.c"):
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
        source_archive = output / f"sqlite-amalgamation-{RELEASE}.zip"
        if args.source_archive is not None:
            shutil.copyfile(args.source_archive, source_archive)
        else:
            with urllib.request.urlopen(SOURCE_URL, timeout=30) as response:
                with source_archive.open("xb") as target:
                    shutil.copyfileobj(response, target)
        source = unpack_source(source_archive, output)
        header = (source / "sqlite3.h").read_text()
        if (f'#define SQLITE_VERSION        "{VERSION}"' not in header
                or f'#define SQLITE_SOURCE_ID      "{SOURCE_ID}"' not in header):
            raise ValueError("SQLite header identity differs from pinned release")
        # Snapshot in-repository inputs so concurrent header edits cannot change
        # the meaning of the saved compiler commands or source hashes.
        includes = output / "scarlet-include"
        shutil.copytree(repo / "user/lib/scarlet-libc/include", includes)
        fixture = output / "main.c"
        shutil.copy2(here / "main.c", fixture)
        vfs = output / "scarlet_vfs.c"
        shutil.copy2(here / "scarlet_vfs.c", vfs)
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
                 "-Werror", *("-D" + option for option in CONFIGURATION)]
        objects = []
        for name in SOURCES:
            obj = output / (Path(name).stem + ".o")
            run([clang, *flags, "-Wno-unused-parameter", "-c", source / name, "-o", obj],
                name + ".log")
            objects.append(obj)
        library = output / "libsqlite3.a"
        run([ar, "rcsD", library, *objects], "ar.log")
        sqlite_members = helper.archive_members(library, machine)
        fixture_obj, binary = output / "main.o", output / "sqlite-probe"
        vfs_obj = output / "scarlet_vfs.o"
        run([clang, *flags, "-c", fixture, "-o", fixture_obj], "fixture.log")
        run([clang, *flags, "-c", vfs, "-o", vfs_obj], "vfs.log")
        link_flags = ["--fix-cortex-a53-843419"] if machine_name == "aarch64" else []
        run([linker, "--gc-sections", "--no-undefined", "-static", "-e", "_start",
             "-z", "max-page-size=4096", *link_flags, "-Map=" + str(output / "link.map"),
             "--trace", crt, fixture_obj, vfs_obj, library, archive, "-o", binary], "link.log")
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
            "scope": "Unmodified SQLite with a Scarlet-native rollback-journal VFS and real libc fd I/O/locks; not the Unix VFS, full POSIX, or a power-loss durability claim.",
            "target": args.target,
            "upstream": {"name": "sqlite", "version": VERSION, "url": SOURCE_URL,
                         "archive_sha256": SOURCE_SHA256, "archive_sha3_256": SOURCE_SHA3_256,
                         "source_id": SOURCE_ID, "source_modified": False},
            "configuration": list(CONFIGURATION),
            "mutex_backend": "SQLITE_CONFIG_MUTEX with real scarlet-libc pthread mutexes; configured before initialization",
            "vfs": {"name": "scarlet-native", "file_methods_version": 1,
                    "concurrency": "exclusive nonblocking inode flock from SHARED until NONE",
                    "wal": False, "mmap": False, "disk_temporary_files": False},
            "expected_exit_code": 53,
            "crash_mode": {"expected_exit_code": 134,
                           "expected_stdout_line": "SCARLET_LIBC_SQLITE_CRASH_READY",
                           "requires_followup": "verify in a fresh process"},
            "expected_stdout_line": "SCARLET_LIBC_SQLITE_OK",
            "create_expected_thread_stdout_line": "SCARLET_LIBC_SQLITE_PTHREAD_OK",
            "guest_execution": "pending",
            "executed_on_scarlet": False,
            "binary": {"path": str(binary), "sha256": sha256(binary)},
            "inputs": {"libc": {"path": str(archive), "sha256": archive_hash},
                       "crt": {"path": str(crt), "sha256": crt_hash},
                       "fixture": {"path": str(fixture), "sha256": sha256(fixture)},
                       "vfs": {"path": str(vfs), "sha256": sha256(vfs)}},
            "header_sha256": {str(path.relative_to(includes)): sha256(path)
                              for path in sorted(includes.rglob("*.h"))},
            "upstream_source_sha256": {name: sha256(source / name) for name in ("sqlite3.c", "sqlite3.h", "sqlite3ext.h", "shell.c")},
            "commands": commands,
            "tool_versions": versions,
            "archives": {"libc": libc_members, "sqlite": sqlite_members},
            "elf": elf_report,
        }
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as error:
        print(f"sqlite consumer build failed: {error}", file=sys.stderr)
        return 1
    print(f"Built and audited {binary}; guest execution remains required.")
    print(f"Run with DIRECTORY create, then DIRECTORY verify in a separate process; require exit 53 and SCARLET_LIBC_SQLITE_OK.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
