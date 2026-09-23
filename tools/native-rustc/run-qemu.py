#!/usr/bin/env python3
"""Boot a staged native Scarlet compiler and require guest compilation plus execution.

Full mode is the default. --frontend-only is a separate diagnostic and never
reports a full PASS. The default ext2 root preserves guest evidence in root.ext2
and avoids copying the complete compiler sysroot into the kernel's CPIO heap.
"""

import argparse
import ipaddress
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import sqlite3
import subprocess
import sys

from audit_elf import Elf

sys.dont_write_bytecode = True
HELPER = Path(__file__).resolve().parents[1] / "loader-smoke" / "run-qemu.py"
spec = importlib.util.spec_from_file_location("loader_smoke_qemu", HELPER)
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
TARGETS = {"aarch64": "aarch64-unknown-scarlet", "riscv64": "riscv64gc-unknown-scarlet"}
HELLO = b"SCARLET_NATIVE_RUSTC_HELLO_OK\n"
MACRO_HELLO = b"SCARLET_NATIVE_PROC_MACRO_OK=42\n"
CARGO_HELLO = b"SCARLET_NATIVE_CARGO_HELLO_OK\n"
CARGO_ONLINE_HELLO = b"SCARLET_NATIVE_CARGO_ONLINE_OK=42\n"
ZLIB_HELLO = b"SCARLET_LIBC_ZLIB_OK"
SQLITE_HELLO = b"SCARLET_LIBC_SQLITE_OK"
SQLITE_PTHREAD_HELLO = b"SCARLET_LIBC_SQLITE_PTHREAD_OK"
SQLITE_CRASH_READY = b"SCARLET_LIBC_SQLITE_CRASH_READY"
SQLITE_JOURNAL_MAGIC = bytes.fromhex("d9d505f920a163d7")
SQLITE_PHASES = {
    f"sqlite-{storage}-{phase}": (134, SQLITE_CRASH_READY) if phase == "crash" else (53, SQLITE_HELLO)
    for storage in ("ext2", "tmpfs") for phase in ("create", "verify", "crash", "recover")
}


def guest_path(value):
    path = PurePosixPath(value)
    if not path.is_absolute() or ".." in path.parts or any(c in value for c in "\r\n\0"):
        raise argparse.ArgumentTypeError("expected an absolute guest path without '..' or control characters")
    return str(path)


def in_root(root, guest):
    path = root / guest.lstrip("/")
    # ELF staging should contain copies, not host symlinks to outside artifacts.
    resolved = path.resolve()
    if root not in resolved.parents:
        raise ValueError(f"staged path escapes its root: {guest}")
    return path


def validate_staging_links(root):
    """Allow packaged relative links only when they resolve inside staging."""
    for path in root.rglob("*"):
        if not path.is_symlink():
            continue
        target = Path(os.readlink(path))
        if target.is_absolute():
            raise ValueError(f"staged symlink must be relative: {path}")
        try:
            resolved = path.resolve(strict=True)
        except (OSError, RuntimeError) as error:
            raise ValueError(f"invalid staged symlink: {path}: {error}") from error
        if root not in resolved.parents:
            raise ValueError(f"staged symlink escapes its root: {path}")


def validate_runtime_copies(sysroot):
    """A loader search directory must not shadow a rebuilt Rust runtime DSO."""
    seen = {}
    for pattern in ("librustc_driver-*.so", "libstd-*.so", "librustc_codegen_*.so"):
        for path in sorted(sysroot.rglob(pattern)):
            digest = hashlib.sha256(path.read_bytes()).digest()
            if path.name in seen and seen[path.name][0] != digest:
                raise ValueError(f"conflicting Rust runtime copies: {seen[path.name][1]} and {path}")
            seen[path.name] = digest, path


def executable(path, arch, label, static=False):
    elf = Elf(path)
    report = elf.report()
    if report["machine"] != arch or report["osabi"] != 83:
        raise ValueError(f"{label} is not a native {arch} Scarlet executable: {path}")
    # The standard RISC-V bootstrap deliberately links _entry at address zero.
    # Validate its actual executable mapping rather than treating zero as absent.
    if not any(header["type"] == 1 and header["flags"] & 1
               and header["vaddr"] <= report["entry"] < header["vaddr"] + header["filesz"]
               for header in elf.headers):
        raise ValueError(f"{label} entry is outside a file-backed executable PT_LOAD: {path}")
    if static and (report["interpreter"] or report["needed"]):
        raise ValueError(f"{label} must be statically linked for bootstrap: {path}")
    return report


def entropy_arguments(arch):
    # Native getrandom requires real entropy. Both kernels discover the MMIO
    # RNG driver unconditionally; use the same free buses as their normal runners.
    bus = 6 if arch == "aarch64" else 5
    return ["-object", "rng-random,id=compiler-entropy,filename=/dev/urandom",
            "-device", f"virtio-rng-device,rng=compiler-entropy,bus=virtio-mmio-bus.{bus}"]


def debugfs_quote(value):
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"') + '"'


def validate_c_startup_evidence(extracted):
    if not (extracted / "C_STARTUP_PASS").is_file():
        raise ValueError("C startup evidence is missing")
    if not (extracted / "c-startup.status").read_text().startswith("exit=Some(43) "):
        raise ValueError("C startup did not exit with the required status 43")


def validate_zlib_evidence(extracted):
    if not (extracted / "ZLIB_PASS").is_file():
        raise ValueError("zlib evidence is missing")
    if not (extracted / "zlib.status").read_text().startswith("exit=Some(47) "):
        raise ValueError("zlib did not exit with the required status 47")
    if ZLIB_HELLO not in (extracted / "zlib.stdout").read_bytes().splitlines():
        raise ValueError("zlib stdout is missing the success marker")


def sqlite_expected_blob():
    """Independent transcription of the C fixture's deterministic BLOB contract."""
    state = 0x735C91A7
    payload = bytearray(131113)
    for index in range(len(payload)):
        state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
        payload[index] = (state >> 24) ^ (index & 255)
    for index in range(65539, 69638):
        payload[index] ^= 0x5A
    return bytes(payload)


def validate_sqlite_database(database):
    if not database.is_file():
        raise ValueError("persisted SQLite database is missing")
    # No journal recovery or mutation of the evidence is allowed on the host.
    for suffix in ("-journal", "-wal", "-shm"):
        sidecar = database.with_name(database.name + suffix)
        if sidecar.exists() and sidecar.stat().st_size:
            raise ValueError(f"persisted SQLite database has an unexpected nonempty {suffix} sidecar")
    try:
        connection = sqlite3.connect(database.resolve().as_uri() + "?mode=ro&immutable=1", uri=True)
        try:
            integrity = connection.execute("PRAGMA integrity_check").fetchall()
            if integrity != [("ok",)]:
                raise ValueError(f"persisted SQLite integrity check failed: {integrity!r}")
            if connection.execute("PRAGMA foreign_key_check").fetchall():
                raise ValueError("persisted SQLite foreign key check failed")
            groups = connection.execute("SELECT id, title FROM groups ORDER BY id").fetchall()
            if groups != [(index, f"group-{index}") for index in range(3)]:
                raise ValueError("persisted SQLite groups differ from the fixture")
            rows = connection.execute("SELECT id, label, score, group_id, payload FROM items ORDER BY id").fetchall()
            payload = sqlite_expected_blob()
            expected = [(index, f"scarlet-{index:02d}-苺", index * 0.25, index % 3,
                         payload if index == 7 else None) for index in range(1, 25)]
            if rows != expected:
                raise ValueError("persisted SQLite rows or BLOB differ from the fixture")
            if connection.execute("PRAGMA index_info(idx_items_group)").fetchall() != [(0, 3, "group_id")]:
                raise ValueError("persisted SQLite group index is missing or incorrect")
        finally:
            connection.close()
    except sqlite3.Error as error:
        raise ValueError(f"persisted SQLite database validation failed: {error}") from error
    return {"database": str(database), "database_sha256": hashlib.sha256(database.read_bytes()).hexdigest(),
            "host_sqlite_version": sqlite3.sqlite_version, "integrity_check": "ok",
            "foreign_key_check": "ok", "group_count": 3, "item_count": 24,
            "score_sum": 75.0, "blob_bytes": len(payload),
            "blob_sha256": hashlib.sha256(payload).hexdigest(), "read_only": True,
            "scope": "ext2 bytes extracted after VM exit; no power-loss durability claim"}


def validate_sqlite_evidence(extracted):
    if not (extracted / "SQLITE_PASS").is_file():
        raise ValueError("SQLite evidence is missing")
    for phase, (expected_exit, marker) in SQLITE_PHASES.items():
        if not (extracted / f"{phase}.status").read_text().startswith(f"exit=Some({expected_exit}) "):
            raise ValueError(f"{phase} did not exit with the required status {expected_exit}")
        if marker not in (extracted / f"{phase}.stdout").read_bytes().splitlines():
            raise ValueError(f"{phase} stdout is missing the success marker")
    journals = {}
    for storage in ("ext2", "tmpfs"):
        if SQLITE_PTHREAD_HELLO not in (extracted / f"sqlite-{storage}-create.stdout").read_bytes().splitlines():
            raise ValueError(f"SQLite {storage} evidence is missing the pthread success marker")
        journal = extracted / f"sqlite-{storage}-crash.journal"
        data = journal.read_bytes()
        if len(data) <= 512 or not data.startswith(SQLITE_JOURNAL_MAGIC):
            raise ValueError(f"SQLite {storage} crash evidence is missing a valid hot rollback journal")
        journals[storage] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    report = validate_sqlite_database(extracted / "sqlite/sqlite-roundtrip.db")
    report.update(process_exit_recovery_verified=True, pthread_verified=True, hot_journal_snapshots=journals)
    return report


def collect_evidence(image, output, guest_output, mode, arch, proc_macro=False, native_fs=False, c_startup=False, zlib=False, sqlite=False, cargo=False, cargo_online=False):
    evidence = output / "guest-evidence"
    evidence.mkdir()
    command = ["debugfs", "-R", f"rdump {debugfs_quote(guest_output)} {debugfs_quote(evidence)}", str(image)]
    with (output / "evidence-extract.log").open("wb") as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=120)
    extracted = evidence / PurePosixPath(guest_output).name
    if c_startup:
        validate_c_startup_evidence(extracted)
    if zlib:
        validate_zlib_evidence(extracted)
    if sqlite:
        validation = validate_sqlite_evidence(extracted)
        (output / "sqlite-host-validation.json").write_text(json.dumps(validation, indent=2) + "\n")
    if cargo:
        if not (extracted / "CARGO_PASS").is_file():
            raise ValueError("native Cargo evidence is missing")
        if not (extracted / "cargo-version.stdout").read_bytes().startswith(b"cargo "):
            raise ValueError("native Cargo version output is missing")
        for phase in ("cargo-version", "cargo-build", "cargo-execute"):
            if not (extracted / f"{phase}.status").read_text().startswith("exit=Some(0) "):
                raise ValueError(f"native Cargo {phase} did not exit successfully")
        if (extracted / "cargo-execute.stdout").read_bytes() != CARGO_HELLO:
            raise ValueError("Cargo-built program stdout differs from the required marker")
        executable(extracted / "cargo-target" / TARGETS[arch] / "release/native-cargo-hello",
                   arch, "Cargo-built guest program")
    if cargo_online:
        if not (extracted / "CARGO_ONLINE_PASS").is_file():
            raise ValueError("native Cargo online evidence is missing")
        for phase in ("cargo-online-build", "cargo-online-execute"):
            if not (extracted / f"{phase}.status").read_text().startswith("exit=Some(0) "):
                raise ValueError(f"native Cargo {phase} did not exit successfully")
        if (extracted / "cargo-online-execute.stdout").read_bytes() != CARGO_ONLINE_HELLO:
            raise ValueError("Cargo online program stdout differs from the required marker")
        executable(extracted / "cargo-online-binary",
                   arch, "Cargo online-built guest program")
        if not any(path.stat().st_size > 0 for path in
                   (extracted / "cargo-online-home" / "registry" / "cache").rglob("itoa-1.0.15.crate")):
            raise ValueError("Cargo online evidence is missing the downloaded itoa crate")
    if native_fs and not (extracted / "NATIVE_FS_PASS").is_file():
        raise ValueError("native filesystem evidence is missing")
    if native_fs and not (extracted / "LIBC_PTHREAD_PASS").is_file():
        raise ValueError("native pthread evidence is missing")
    if mode == "full":
        if not (extracted / "PASS").is_file():
            raise ValueError("full marker appeared without persisted guest PASS evidence")
        if (extracted / "execute.stdout").read_bytes() != HELLO:
            raise ValueError("persisted generated-program stdout differs from the required marker")
        if not (extracted / "execute.status").read_text().startswith("exit=Some(37) "):
            raise ValueError("persisted generated-program exit status is not 37")
        executable(extracted / "hello", arch, "generated guest hello")
        if proc_macro:
            if not (extracted / "PROC_MACRO_PASS").is_file():
                raise ValueError("proc macro evidence is missing")
            if (extracted / "proc-macro-execute.stdout").read_bytes() != MACRO_HELLO:
                raise ValueError("proc macro program stdout differs from the required marker")
            for phase in ("proc-macro-build", "proc-macro-build-other", "proc-macro-expand", "proc-macro-execute"):
                if not (extracted / f"{phase}.status").read_text().startswith("exit=Some(0) "):
                    raise ValueError(f"{phase} did not exit successfully")
            executable(extracted / "macro-app", arch, "generated proc macro consumer")
            for name in ("libscarlet_probe_macros.so", "libscarlet_probe_macros_other.so"):
                macro = Elf(extracted / name).report()
                if macro["elf_type"] != "DYN" or macro["machine"] != arch or macro["osabi"] != 83:
                    raise ValueError("proc macro is not a native Scarlet shared object")
    elif not (extracted / "FRONTEND_PASS").is_file():
        raise ValueError("frontend marker appeared without persisted guest diagnostic evidence")
    return str(extracted)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    parser.add_argument("--arch", choices=TARGETS, default="aarch64")
    parser.add_argument("--kernel", type=Path, required=True, help="fresh matching Scarlet Limine kernel")
    parser.add_argument("--staging", type=Path, required=True, help="native sysroot overlay produced by stage.py")
    parser.add_argument("--bootstrap", type=Path, required=True, help="standard user/bin native static init")
    parser.add_argument("--probe", type=Path, help="override the staged native-rustc-probe with a fresh build")
    parser.add_argument("--cargo", type=Path, help="cross-built Scarlet-native Cargo executable to test inside the guest")
    parser.add_argument("--cargo-online", action="store_true", help="also fetch itoa from crates.io over HTTPS and build it on Scarlet")
    parser.add_argument("--resolverd", type=Path, help="Scarlet-native resolverd executable required for --cargo-online")
    parser.add_argument("--ca-bundle", type=Path, help="OS CA bundle to install for the online Cargo guest")
    parser.add_argument("--dns-server", type=ipaddress.IPv4Address,
                        default=ipaddress.IPv4Address("10.0.2.3"),
                        help="IPv4 nameserver for the online guest; defaults to QEMU's resolver")
    parser.add_argument("--c-startup-probe", type=Path, help="also execute a static C main + Scarlet CRT + std-backed libc fixture (expected exit 43)")
    parser.add_argument("--zlib-probe", type=Path, help="also execute the upstream zlib C consumer (expected exit 47 and success marker)")
    parser.add_argument("--sqlite-probe", type=Path, help="also execute separate create/verify/crash/recover SQLite processes on ext2 and tmpfs, then verify the extracted ext2 database on the host")
    parser.add_argument("--output", type=Path, required=True, help="NEW private artifacts directory")
    parser.add_argument("--rustc", type=guest_path, default="/opt/native-rustc/bin/rustc")
    parser.add_argument("--sysroot", type=guest_path, default="/opt/native-rustc")
    parser.add_argument("--linker", type=guest_path, help="native Scarlet linker executable already in staging; required in full mode")
    parser.add_argument("--linker-flavor", help="rustc -C linker-flavor, e.g. ld.lld for direct native Wild/LLD")
    parser.add_argument("--backend", type=guest_path, help="native codegen backend DSO already in staging")
    parser.add_argument("--frontend-only", action="store_true", help="diagnose version/cfg/frontend only; never a full success")
    parser.add_argument("--dummy", action="store_true", help="dummy backend, valid only with --frontend-only")
    parser.add_argument("--proc-macro", action="store_true", help="also compile and load function-like, attribute and derive macros")
    parser.add_argument("--native-fs", action="store_true", help="also run C ABI and Rust filesystem checks; requires the native-fs probe feature")
    parser.add_argument("--phase-timeout", type=int, default=900, help="guest compiler timeout per phase, seconds")
    parser.add_argument("--run-timeout", type=int, default=60, help="guest generated-program timeout, seconds")
    parser.add_argument("--timeout", type=float, default=3900, help="outer VM timeout; also cleans up unkillable guest children")
    parser.add_argument("--memory", default="4G", help="QEMU guest RAM (does not change kernel heap capacity)")
    parser.add_argument("--cpus", type=int, default=1)
    parser.add_argument("--accel", choices=("tcg", "hvf"), default="tcg",
                        help="QEMU accelerator; hvf is available for AArch64 guests on Apple Silicon")
    parser.add_argument("--storage", choices=("ext2", "initramfs"), default="ext2")
    parser.add_argument("--disk-size-mib", type=int, default=0, help="private ext2 size; 0 reserves payload plus at least 1 GiB")
    parser.add_argument("--root-device", type=guest_path, default="/dev/vblk1", help="kernel path for the second attached virtio block disk")
    parser.add_argument("--limine-cache", type=Path, help="existing Limine cache to copy into the private output")
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    if not args.frontend_only and not args.linker:
        parser.error("full mode requires --linker naming a native executable in --staging")
    if args.dummy and (not args.frontend_only or args.backend):
        parser.error("--dummy requires --frontend-only and cannot be combined with --backend")
    if args.frontend_only and (args.linker or args.linker_flavor):
        parser.error("linker options require full mode")
    if args.proc_macro and args.frontend_only:
        parser.error("--proc-macro requires full mode")
    if args.cargo and args.frontend_only:
        parser.error("--cargo requires full mode")
    if args.cargo and args.storage != "ext2":
        parser.error("--cargo requires ext2 storage for persisted build evidence")
    if args.cargo_online and (not args.cargo or not args.resolverd or args.frontend_only):
        parser.error("--cargo-online requires --cargo, --resolverd and full mode")
    if args.cargo_online and not args.ca_bundle:
        parser.error("--cargo-online requires --ca-bundle")
    if args.ca_bundle and not args.cargo_online:
        parser.error("--ca-bundle requires --cargo-online")
    if args.resolverd and not args.cargo_online:
        parser.error("--resolverd requires --cargo-online")
    if (args.c_startup_probe or args.zlib_probe or args.sqlite_probe) and args.storage != "ext2":
        parser.error("C startup, zlib and SQLite probes require ext2 to verify persisted evidence")
    if not all(1 <= seconds <= 86400 for seconds in (args.phase_timeout, args.run_timeout)):
        parser.error("guest timeouts must be between 1 and 86400 seconds")
    if not math.isfinite(args.timeout) or args.timeout <= 0 or args.cpus < 1 or args.disk_size_mib < 0:
        parser.error("invalid timeout, CPU count, or disk size")
    if args.accel == "hvf" and args.arch != "aarch64":
        parser.error("--accel hvf is supported only for an AArch64 guest")
    if not re.fullmatch(r"[1-9][0-9]*[MG]", args.memory):
        parser.error("--memory must be a positive integer followed by M or G")
    kernel, staging, bootstrap, output = (p.resolve() for p in (args.kernel, args.staging, args.bootstrap, args.output))
    if output.exists():
        parser.error("--output already exists; use a new directory so old PASS evidence cannot survive")
    if args.limine_cache:
        source_cache = args.limine_cache.resolve()
        if source_cache == output or source_cache in output.parents or output in source_cache.parents:
            parser.error("--limine-cache and --output must not overlap")
    if output == staging or staging in output.parents or output in staging.parents:
        parser.error("--output and --staging must not overlap")
    if not kernel.is_file() or not staging.is_dir():
        parser.error("kernel or staging does not exist")
    try:
        validate_staging_links(staging)
        validate_runtime_copies(in_root(staging, args.sysroot))
    except ValueError as error:
        parser.error(str(error))
    qemu = f"qemu-system-{args.arch}"
    required_tools = [qemu, "cargo-scarlet-plugin-limine"]
    if args.storage == "ext2":
        required_tools += ["mkfs.ext2", "debugfs"]
    for tool in required_tools:
        if shutil.which(tool) is None:
            parser.error(f"{tool} is missing; run inside the Scarlet development shell")
    # These checks are performed even for prepare-only: a cross-compiler on the
    # host cannot silently replace the native compiler being tested.
    compiler_report = executable(in_root(staging, args.rustc), args.arch, "rustc")
    if compiler_report["interpreter"] != "/bin/scarlet-ld":
        parser.error("staged native rustc must use /bin/scarlet-ld")
    executable(bootstrap, args.arch, "bootstrap init", static=True)
    executable(in_root(staging, "/bin/scarlet-ld"), args.arch, "loader", static=True)
    probe = args.probe.resolve() if args.probe else in_root(staging, "/system/bin/native-rustc-probe")
    executable(probe, args.arch, "guest probe")
    cargo = args.cargo.resolve() if args.cargo else None
    if cargo:
        cargo_report = executable(cargo, args.arch, "native Cargo")
        if cargo_report["interpreter"] not in ("/bin/scarlet-ld", "/system/bin/scarlet-ld"):
            parser.error("native Cargo must use /bin/scarlet-ld (or the temporary compatibility path)")
    resolverd = args.resolverd.resolve() if args.resolverd else None
    if resolverd:
        resolver_report = executable(resolverd, args.arch, "native resolverd")
        if resolver_report["interpreter"] not in ("/bin/scarlet-ld", "/system/bin/scarlet-ld"):
            parser.error("native resolverd must use /bin/scarlet-ld (or the temporary compatibility path)")
    ca_bundle = args.ca_bundle.resolve() if args.ca_bundle else None
    if ca_bundle and (not ca_bundle.is_file() or b"-----BEGIN CERTIFICATE-----" not in ca_bundle.read_bytes()):
        parser.error("--ca-bundle must name a PEM CA certificate bundle")
    c_startup = args.c_startup_probe.resolve() if args.c_startup_probe else None
    if c_startup:
        executable(c_startup, args.arch, "C startup probe", static=True)
    zlib = args.zlib_probe.resolve() if args.zlib_probe else None
    if zlib:
        executable(zlib, args.arch, "zlib consumer", static=True)
    sqlite = args.sqlite_probe.resolve() if args.sqlite_probe else None
    if sqlite:
        executable(sqlite, args.arch, "SQLite consumer", static=True)
    if args.linker:
        executable(in_root(staging, args.linker), args.arch, "linker")
    if args.backend:
        backend = Elf(in_root(staging, args.backend)).report()
        if backend["machine"] != args.arch or backend["elf_type"] != "DYN":
            parser.error("--backend must name a matching native shared library")
    if not list(in_root(staging, args.sysroot).glob(f"lib/rustlib/{TARGETS[args.arch]}/lib/libstd-*.rlib")):
        parser.error("staged native sysroot is missing target libstd rlib")
    if args.arch == "aarch64":
        code_names = ("SCARLET_EFI_CODE_ARM64_EL2", "SCARLET_EFI_CODE_ARM64")
        variable_names = ("SCARLET_EFI_VARS_ARM64_EL2", "SCARLET_EFI_VARS_ARM64")
        if args.accel == "hvf":
            code_names = ("SCARLET_EFI_CODE_ARM64_HVF", *code_names)
            variable_names = ("SCARLET_EFI_VARS_ARM64_HVF", *variable_names)
        code = smoke.firmware(code_names)
        variables = smoke.firmware(variable_names)
        cpu = "host" if args.accel == "hvf" else "max"
        machine = ["-machine", "virt,gic-version=3,acpi=off", "-cpu", cpu]
        boot_device = "virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0"
        root_device = "virtio-blk-device,drive=root,bus=virtio-mmio-bus.1"
        gpu_device = "virtio-gpu-device,bus=virtio-mmio-bus.3"
        console = "ttyAMA0"
    else:
        code = smoke.firmware(("SCARLET_EFI_CODE_RV64",))
        variables = smoke.firmware(("SCARLET_EFI_VARS_RV64",))
        machine = ["-machine", "virt,acpi=off", "-bios", "default"]
        boot_device = "virtio-blk-pci,drive=boot,bus=pcie.0,addr=0x1"
        root_device = "virtio-blk-pci,drive=root,bus=pcie.0,addr=0x2"
        gpu_device = "virtio-gpu-device,bus=virtio-mmio-bus.1"
        console = "ttyS0"
    output.mkdir(parents=True)
    mode = "frontend" if args.frontend_only else "full"
    result_path = output / "result.json"
    result_path.write_text(json.dumps({"result": "PREPARING", "mode": mode}) + "\n")
    root = output / "root-tree"
    shutil.copytree(staging, root, symlinks=True)
    shutil.copy2(bootstrap, root / "init")
    shutil.copy2(probe, root / "system/bin/native-rustc-probe")
    if cargo:
        shutil.copy2(cargo, root / "opt/native-rustc/bin/cargo")
    if resolverd:
        shutil.copy2(resolverd, root / "opt/native-rustc/bin/resolverd")
        (root / "etc/resolv.conf").write_text(f"nameserver {args.dns_server}\n")
    if ca_bundle:
        (root / "etc/ssl/certs").mkdir(parents=True, exist_ok=True)
        shutil.copy2(ca_bundle, root / "etc/ssl/certs/ca-certificates.crt")
    if c_startup:
        shutil.copy2(c_startup, root / "system/bin/native-c-startup-probe")
    if zlib:
        shutil.copy2(zlib, root / "system/bin/native-zlib-probe")
    if sqlite:
        shutil.copy2(sqlite, root / "system/bin/native-sqlite-probe")
    for directory in ("dev/pts", "mnt/newroot", "etc", "root", "old_root", "tmp"):
        (root / directory).mkdir(parents=True, exist_ok=True)
    guest_output = "/native-rustc-output" if args.storage == "ext2" else "/tmp/native-rustc-output"
    if (root / guest_output.lstrip("/")).exists():
        raise ValueError("staging already contains the reserved guest output directory")
    probe_args = [args.rustc, args.sysroot, TARGETS[args.arch], guest_output,
                  "--timeout", str(args.phase_timeout), "--run-timeout", str(args.run_timeout)]
    if not args.frontend_only:
        probe_args += ["--full", "--linker", args.linker]
    if args.linker_flavor:
        probe_args += ["--linker-flavor", args.linker_flavor]
    if args.backend:
        probe_args += ["--backend", args.backend]
    if args.dummy:
        probe_args += ["--dummy"]
    if args.proc_macro:
        probe_args += ["--proc-macro"]
    if cargo:
        probe_args += ["--cargo", "/opt/native-rustc/bin/cargo"]
    if resolverd:
        probe_args += ["--cargo-online", "--resolverd", "/opt/native-rustc/bin/resolverd"]
    if args.native_fs:
        probe_args += ["--native-fs"]
    if c_startup:
        probe_args += ["--c-startup", "/system/bin/native-c-startup-probe"]
    if zlib:
        probe_args += ["--zlib", "/system/bin/native-zlib-probe"]
    if sqlite:
        probe_args += ["--sqlite", "/system/bin/native-sqlite-probe"]
    if any(any(char in arg for char in "\r\n\0") for arg in probe_args):
        raise ValueError("probe arguments cannot contain line breaks or NUL")
    (root / "etc/native-rustc-probe.args").write_text("\n".join(probe_args) + "\n")
    archive_root = root
    root_image = None
    image_commands = []
    cmdline = f"console={console} init=/init init.exec=/system/bin/native-rustc-probe init.console=/dev/tty0"
    if args.storage == "ext2":
        archive_root = output / "bootstrap-tree"
        for directory in ("dev", "mnt/newroot"):
            (archive_root / directory).mkdir(parents=True, exist_ok=True)
        shutil.copy2(bootstrap, archive_root / "init")
        size = sum(p.stat().st_size for p in root.rglob("*") if p.is_file())
        minimum_mib = (size * 2 + 1048575) // 1048576 + 1024
        disk_mib = args.disk_size_mib or minimum_mib
        if disk_mib * 1048576 < size + 268435456:
            raise ValueError("--disk-size-mib must leave at least 256 MiB beyond staged file bytes")
        root_image = output / "root.ext2"
        with root_image.open("wb") as disk:
            disk.truncate(disk_mib * 1048576)
        make_ext2 = ["mkfs.ext2", "-F", "-b", "4096", "-L", "NATIVE_RUSTC", "-d", str(root), str(root_image)]
        image_commands.append(make_ext2)
        with (output / "root-image-build.log").open("wb") as log:
            subprocess.run(make_ext2, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=300)
        cmdline += f" root={args.root_device} rootfstype=ext2 rootwait"
    if args.cargo_online:
        cmdline += " net.ip=10.0.2.15 net.mask=255.255.255.0 net.gw=10.0.2.2"
    else:
        payload = sum(p.stat().st_size for p in root.rglob("*") if p.is_file())
        if payload >= 256 * 1048576:
            raise ValueError("initramfs payload is >=256 MiB; use --storage ext2 to preserve kernel heap headroom")
    archive = output / "initramfs.cpio"
    boot_image = output / "boot.img"
    cache = output / "limine-cache"
    if args.limine_cache:
        source_cache = args.limine_cache.resolve()
        if output == source_cache or source_cache in output.parents or output in source_cache.parents:
            raise ValueError("Limine cache must be outside the new output directory")
        shutil.copytree(source_cache, cache)
    smoke.create_initramfs(archive_root, archive)
    plugin_command = ["cargo-scarlet-plugin-limine", "--arch", args.arch, "--kernel", str(kernel),
                      "--initramfs", str(archive), "--output", str(boot_image), "--cache-dir", str(cache),
                      "--cmdline", cmdline]
    image_commands.append(plugin_command)
    with (output / "image-build.log").open("wb") as log:
        subprocess.run(plugin_command, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    runtime_variables = output / "efi-vars.fd"
    shutil.copyfile(variables, runtime_variables)
    runtime_variables.chmod(0o600)
    command = [qemu, *machine, "-m", args.memory, "-accel", args.accel, "-smp", str(args.cpus), "-no-reboot",
               "-display", "none", "-monitor", "none", "-serial", "stdio",
               "-drive", f"if=pflash,format=raw,unit=0,file={smoke.qemu_filename(code)},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={smoke.qemu_filename(runtime_variables)}",
               "-global", "virtio-mmio.force-legacy=false",
               "-drive", f"id=boot,file={smoke.qemu_filename(boot_image)},format=raw,if=none",
               "-device", boot_device]
    if root_image:
        command += ["-drive", f"id=root,file={smoke.qemu_filename(root_image)},format=raw,if=none", "-device", root_device]
    command += ["-device", gpu_device, *entropy_arguments(args.arch)]
    if args.cargo_online:
        command += ["-netdev", "user,id=net0", "-device", "virtio-net-pci,netdev=net0,bus=pcie.0"]
    (output / "commands.json").write_text(json.dumps({"image": image_commands, "qemu": command, "probe": probe_args}, indent=2) + "\n")
    if args.prepare_only:
        result_path.write_text(json.dumps({"result": "PREPARED", "mode": mode, "executed_on_scarlet": False}) + "\n")
        print(f"Prepared native rustc {mode} boot artifacts: {output}")
        return 0
    smoke.SUCCESS = re.compile(rb"\nNATIVE_RUSTC " + (b"FRONTEND" if args.frontend_only else b"FULL") + rb" PASS\r?\n")
    smoke.FAILURE = re.compile(rb"\nNATIVE_RUSTC FAIL(?:[ :\r\n]|$)")
    result = smoke.run_guest(command, output, args.timeout)
    result.update(mode=mode, full_compilation_verified=False, proc_macro_verified=False, native_fs_verified=False, c_startup_verified=False, zlib_verified=False, sqlite_verified=False, sqlite_disk_verified=False, cargo_verified=False, cargo_online_verified=False)
    succeeded = result["result"] == "PASS"
    if root_image:
        try:
            if succeeded:
                result["guest_evidence"] = collect_evidence(root_image, output, guest_output, mode, args.arch, args.proc_macro, args.native_fs, bool(c_startup), bool(zlib), bool(sqlite), bool(cargo), args.cargo_online)
            else:
                # Preserve partial logs for failed compiler or bootstrap attempts.
                evidence = output / "guest-evidence"
                evidence.mkdir()
                with (output / "evidence-extract.log").open("wb") as log:
                    subprocess.run(["debugfs", "-R", f"rdump {debugfs_quote(guest_output)} {debugfs_quote(evidence)}", str(root_image)],
                                   stdout=log, stderr=subprocess.STDOUT, timeout=120)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            result["evidence_error"] = str(error)
            if succeeded:
                result["result"] = "persisted evidence validation failed"
                succeeded = False
    if succeeded:
        result["result"] = "FULL_PASS" if mode == "full" else "FRONTEND_PASS"
        result["full_compilation_verified"] = mode == "full"
        result["proc_macro_verified"] = args.proc_macro
        result["native_fs_verified"] = args.native_fs
        result["c_startup_verified"] = bool(c_startup)
        result["zlib_verified"] = bool(zlib)
        result["sqlite_verified"] = bool(sqlite)
        result["sqlite_disk_verified"] = bool(sqlite)
        result["cargo_verified"] = bool(cargo)
        result["cargo_online_verified"] = args.cargo_online
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"\nNative rustc ({mode}): {result['result']} (artifacts: {output})")
    return 0 if succeeded else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Native rustc harness error: {error}", file=sys.stderr)
        sys.exit(1)
