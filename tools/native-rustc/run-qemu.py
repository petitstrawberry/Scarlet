#!/usr/bin/env python3
"""Boot a staged native Scarlet compiler and require guest compilation plus execution.

Full mode is the default. --frontend-only is a separate diagnostic and never
reports a full PASS. The default ext2 root preserves guest evidence in root.ext2
and avoids copying the complete compiler sysroot into the kernel's CPIO heap.
"""

import argparse
import importlib.util
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import shutil
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


def collect_evidence(image, output, guest_output, mode, arch):
    evidence = output / "guest-evidence"
    evidence.mkdir()
    command = ["debugfs", "-R", f"rdump {debugfs_quote(guest_output)} {debugfs_quote(evidence)}", str(image)]
    with (output / "evidence-extract.log").open("wb") as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=120)
    extracted = evidence / PurePosixPath(guest_output).name
    if mode == "full":
        if not (extracted / "PASS").is_file():
            raise ValueError("full marker appeared without persisted guest PASS evidence")
        if (extracted / "execute.stdout").read_bytes() != HELLO:
            raise ValueError("persisted generated-program stdout differs from the required marker")
        if not (extracted / "execute.status").read_text().startswith("exit=Some(37) "):
            raise ValueError("persisted generated-program exit status is not 37")
        executable(extracted / "hello", arch, "generated guest hello")
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
    parser.add_argument("--output", type=Path, required=True, help="NEW private artifacts directory")
    parser.add_argument("--rustc", type=guest_path, default="/opt/native-rustc/bin/rustc")
    parser.add_argument("--sysroot", type=guest_path, default="/opt/native-rustc")
    parser.add_argument("--linker", type=guest_path, help="native Scarlet linker executable already in staging; required in full mode")
    parser.add_argument("--linker-flavor", help="rustc -C linker-flavor, e.g. ld.lld for direct native Wild/LLD")
    parser.add_argument("--backend", type=guest_path, help="native codegen backend DSO already in staging")
    parser.add_argument("--frontend-only", action="store_true", help="diagnose version/cfg/frontend only; never a full success")
    parser.add_argument("--dummy", action="store_true", help="dummy backend, valid only with --frontend-only")
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
    if compiler_report["interpreter"] != "/system/bin/scarlet-ld":
        parser.error("staged native rustc must use /system/bin/scarlet-ld")
    executable(bootstrap, args.arch, "bootstrap init", static=True)
    executable(in_root(staging, "/system/bin/scarlet-ld"), args.arch, "loader", static=True)
    probe = args.probe.resolve() if args.probe else in_root(staging, "/system/bin/native-rustc-probe")
    executable(probe, args.arch, "guest probe")
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
    (output / "commands.json").write_text(json.dumps({"image": image_commands, "qemu": command, "probe": probe_args}, indent=2) + "\n")
    if args.prepare_only:
        result_path.write_text(json.dumps({"result": "PREPARED", "mode": mode, "executed_on_scarlet": False}) + "\n")
        print(f"Prepared native rustc {mode} boot artifacts: {output}")
        return 0
    smoke.SUCCESS = re.compile(rb"\nNATIVE_RUSTC " + (b"FRONTEND" if args.frontend_only else b"FULL") + rb" PASS\r?\n")
    smoke.FAILURE = re.compile(rb"\nNATIVE_RUSTC FAIL(?:[ :\r\n]|$)")
    result = smoke.run_guest(command, output, args.timeout)
    result.update(mode=mode, full_compilation_verified=False)
    succeeded = result["result"] == "PASS"
    if root_image:
        try:
            if succeeded:
                result["guest_evidence"] = collect_evidence(root_image, output, guest_output, mode, args.arch)
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
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"\nNative rustc ({mode}): {result['result']} (artifacts: {output})")
    return 0 if succeeded else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Native rustc harness error: {error}", file=sys.stderr)
        sys.exit(1)
