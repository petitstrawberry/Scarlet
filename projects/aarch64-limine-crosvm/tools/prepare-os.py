#!/usr/bin/env python3
"""Stage an OS guest and a dedicated native crosvm launcher for run.py."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

PROJECT = Path(__file__).resolve().parents[1]
ROOT = PROJECT.parents[1]
SOURCE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("crosvm_fixture", Path(__file__).with_name("fixture.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-staging", type=Path, default=PROJECT / ".scarlet/base/staging")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--image", type=Path, required=True)
    ramdisk = parser.add_mutually_exclusive_group(required=True)
    ramdisk.add_argument("--initrd", type=Path)
    ramdisk.add_argument("--busybox", type=Path, help="static AArch64 BusyBox for the Linux fixture")
    parser.add_argument("--mem", type=int, default=512)
    parser.add_argument("--serial", action="append", help="crosvm serial configuration; may repeat")
    parser.add_argument("--params", default="earlycon=uart8250,mmio,0x3f8 console=ttyS0 rdinit=/init nokaslr loglevel=7 panic=-1")
    parser.add_argument("--disk", type=Path, action="append", default=[], help="copy a writable guest disk; may repeat")
    parser.add_argument("--read-only-disk", type=Path, action="append", default=[],
                        help="copy a read-only guest disk, attached before writable disks")
    parser.add_argument("--block-check", action="store_true", help="create a private virtio-block read/write fixture")
    args = parser.parse_args()
    base, output = args.base_staging.resolve(), args.output.resolve()
    if output == base or base in output.parents or output in base.parents:
        parser.error("output and base staging must be separate directories")
    disks = [(p, True) for p in args.read_only_disk] + [(p, False) for p in args.disk]
    if args.mem <= 0 or (args.block_check and (not args.busybox or disks)):
        parser.error("positive memory required; block-check requires BusyBox and no --disk")
    for p in [base / "init", base / "bin/scarlet-ld", args.image, args.initrd or args.busybox,
              *(p for p, _ in disks)]:
        if not p.is_file():
            parser.error(f"missing input: {p}")
    output.mkdir(parents=True, exist_ok=True)
    staging = output / "staging"
    shutil.copytree(base, staging, dirs_exist_ok=True, symlinks=True)
    guest = staging / "systems/linux-aarch64/guest"
    guest.mkdir(parents=True, exist_ok=True)
    shutil.copy2(args.image, guest / "Image")
    if args.initrd:
        shutil.copy2(args.initrd, guest / "initramfs.cpio")
    else:
        ramdisk_root = output / "initramfs-root"
        for name in ["bin", "dev", "proc", "sys", "tmp", "mnt"]:
            (ramdisk_root / name).mkdir(parents=True, exist_ok=True)
        shutil.copy2(args.busybox, ramdisk_root / "bin/busybox")
        (ramdisk_root / "bin/busybox").chmod(0o755)
        shell = ramdisk_root / "bin/sh"
        shell.unlink(missing_ok=True)
        shell.symlink_to("busybox")
        shutil.copy2(PROJECT / "fixtures/linux-init.sh", ramdisk_root / "init")
        (ramdisk_root / "init").chmod(0o755)
        smoke.create_initramfs(ramdisk_root, guest / "initramfs.cpio")
    disk_names = []
    for index, (source, readonly) in enumerate(disks):
        name = f"disk-{index}.img"
        shutil.copy2(source, guest / name)
        disk_names.append((name, readonly))
    params = args.params
    if args.block_check:
        disk_root = output / "block-input"
        disk_root.mkdir(exist_ok=True)
        (disk_root / "input").write_text("virtio-input\n")
        with (guest / "data.ext2").open("wb") as disk:
            disk.truncate(16 * 1024 * 1024)
        subprocess.run(["mkfs.ext2", "-F", "-d", str(disk_root), str(guest / "data.ext2")], check=True)
        disk_names.append(("data.ext2", False))
        params += " scarlet.block_check=1"
    command = ["/usr/bin/crosvm", "--no-syslog", "--log-level", "debug", "run",
               "--disable-sandbox", "--no-pmu", "--no-rng", "--no-usb", "--cpus", "1",
               "--mem", str(args.mem), "--initrd", "/guest/initramfs.cpio", "--params", params]
    for serial in args.serial or ["type=stdout,hardware=serial,num=1,console=true,stdin=false"]:
        command += ["--serial", serial]
    for name, readonly in disk_names:
        command += ["--block", f"path=/guest/{name},ro={str(readonly).lower()},lock=false"]
    command += ["/guest/Image"]
    (output / "crosvm.args").write_text("\n".join(command) + "\n")
    environment = dict(os.environ, SCARLET_CROSVM_ARGS="\n".join(command))
    subprocess.run(["cargo", "build", "--manifest-path", str(PROJECT / "init/Cargo.toml"),
                    "--target", "aarch64-unknown-scarlet", "--release", "--target-dir", str(output / "init")],
                   env=environment, check=True)
    shutil.copy2(output / "init/aarch64-unknown-scarlet/release/crosvm-smoke-init", staging / "init")
    (output / "profile.json").write_text(json.dumps({"image": str(args.image.resolve()),
        "initrd": str(args.initrd.resolve()) if args.initrd else None,
        "busybox": str(args.busybox.resolve()) if args.busybox else None,
        "disks": [{"path": str(p.resolve()), "readonly": ro} for p, ro in disks],
        "argv": command}, indent=2) + "\n")
    smoke.activate_profile(output)
    print(staging)


if __name__ == "__main__":
    main()
