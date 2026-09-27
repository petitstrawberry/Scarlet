#!/usr/bin/env python3
"""Run an AArch64 crosvm guest under Scarlet in an isolated QEMU/TCG instance."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

PROJECT = Path(__file__).resolve().parents[1]
ROOT = PROJECT.parents[1]
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("crosvm_fixture", Path(__file__).with_name("fixture.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, help="kernel for private image preparation")
    parser.add_argument("--boot-image", type=Path, help="existing project boot image")
    parser.add_argument("--rootfs-image", type=Path, help="existing project root filesystem")
    parser.add_argument("--staging", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--success-marker", default="SCARLET_CROSVM_GUEST_OK",
                        help="exact guest serial line required for PASS")
    parser.add_argument("--gdb-socket", type=Path, help="optional private Unix GDB socket")
    parser.add_argument("--prepare-only", action="store_true", help="write images and commands without starting QEMU")
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    if not args.success_marker or any(c in args.success_marker for c in "\r\n"):
        parser.error("success marker must be one nonempty line")
    if bool(args.boot_image) != bool(args.rootfs_image):
        parser.error("--boot-image and --rootfs-image must be supplied together")
    if bool(args.kernel) == bool(args.boot_image):
        parser.error("supply either --kernel or the existing boot/rootfs image pair")
    staging, output = (p.resolve() for p in (args.staging, args.output))
    kernel = args.kernel.resolve() if args.kernel else None
    if output == staging or staging in output.parents:
        parser.error("output must be outside staging")
    for path in ([kernel] if kernel else [args.boot_image, args.rootfs_image]) + [staging / "init", staging / "bin/scarlet-ld", staging / "systems/linux-aarch64/usr/bin/crosvm",
                 staging / "systems/linux-aarch64/guest/Image"]:
        if not path.is_file():
            parser.error(f"missing input: {path}")
    output.mkdir(parents=True, exist_ok=True)
    (output / "result.json").write_text('{"result":"PREPARING"}\n')
    code = smoke.firmware(("SCARLET_EFI_CODE_ARM64_EL2", "SCARLET_EFI_CODE_ARM64"))
    variables = smoke.firmware(("SCARLET_EFI_VARS_ARM64_EL2", "SCARLET_EFI_VARS_ARM64"))
    build = None
    if args.boot_image:
        image, rootfs = args.boot_image.resolve(), args.rootfs_image.resolve()
    else:
        archive, image = output / "initramfs.cpio", output / "boot.img"
        # CpioFS intentionally has no mmap support. The Linux loader and crosvm's
        # guest image mappings need the same ext2 backing as a normal Scarlet root.
        rootfs = output / "rootfs.ext2"
        payload_bytes = sum(p.stat().st_size for p in staging.rglob("*") if p.is_file())
        mib = 1024 * 1024
        rootfs_bytes = max(128 * mib, (payload_bytes * 5 // 4 + 64 * mib + mib - 1) // mib * mib)
        with rootfs.open("wb") as disk:
            disk.truncate(rootfs_bytes)
        with (output / "rootfs-build.log").open("wb") as log:
            subprocess.run(["mkfs.ext2", "-F", "-d", str(staging), str(rootfs)],
                           stdout=log, stderr=subprocess.STDOUT, check=True)
        # Guest disks belong on ext2; embedding them again in the boot initramfs
        # would consume guest RAM before crosvm has even allocated its VM memory.
        with tempfile.TemporaryDirectory(prefix="crosvm-boot-", dir=output) as directory:
            boot = Path(directory)
            for name in ["bin", "dev", "mnt/newroot", "home", "shared", "tmp", "root"]:
                (boot / name).mkdir(parents=True, exist_ok=True)
            for name in ["init", "bin/scarlet-ld"]:
                shutil.copy2(staging / name, boot / name)
            smoke.create_initramfs(boot, archive)
        build = ["cargo-scarlet-plugin-limine", "--arch", "aarch64", "--kernel", str(kernel),
                 "--initramfs", str(archive), "--output", str(image),
                 "--cache-dir", str(output / "cache"), "--cmdline", "console=ttyAMA0 init=/init"]
        with (output / "image-build.log").open("wb") as log:
            subprocess.run(build, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    runtime_vars = output / "efi-vars.fd"
    shutil.copyfile(variables, runtime_vars)
    runtime_vars.chmod(0o600)
    command = ["qemu-system-aarch64", "-machine", "virt,gic-version=3,acpi=off,virtualization=on",
               "-cpu", "max", "-m", "2G", "-smp", "1", "-accel", "tcg", "-no-reboot",
               "-display", "none", "-monitor", "none", "-serial", "stdio",
               "-drive", f"if=pflash,format=raw,unit=0,file={smoke.qemu_filename(code)},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={smoke.qemu_filename(runtime_vars)}",
               "-global", "virtio-mmio.force-legacy=false",
               "-drive", f"id=boot,file={smoke.qemu_filename(image)},format=raw,if=none",
               "-device", "virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0",
               "-drive", f"id=root,file={smoke.qemu_filename(rootfs)},format=raw,if=none",
               "-device", "virtio-blk-device,drive=root,bus=virtio-mmio-bus.1",
               "-device", "virtio-rng-device,bus=virtio-mmio-bus.2", "-nic", "none"]
    if args.gdb_socket:
        command += ["-gdb", f"unix:{args.gdb_socket},server=on,wait=off"]
    (output / "commands.json").write_text(json.dumps({"image": build, "qemu": command}, indent=2) + "\n")
    if args.prepare_only:
        (output / "result.json").write_text('{"result":"PREPARED"}\n')
        print(f"Prepared crosvm artifacts: {output}")
        return 0
    # Both Linux's UART console and Scarlet's tty may translate LF to CRLF.
    smoke.SUCCESS = re.compile(rb"\n" + re.escape(args.success_marker.encode()) + rb"\r*\n")
    smoke.FAILURE = re.compile(rb"SCARLET_CROSVM_FAIL(?:[ :\r\n]|$)|exiting with error|error while loading shared libraries|stack smashing detected")
    result = smoke.run_guest(command, output, args.timeout)
    (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"\ncrosvm smoke: {result['result']}")
    return 0 if result["result"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
