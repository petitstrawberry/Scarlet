#!/usr/bin/env python3
"""Run Bionic directly on Scarlet's Android ABI in a private AArch64 QEMU image.

No Android/Linux guest kernel is involved. An optional runtime root must contain
real Android runtime files (NDK .so files are link stubs, not runtime libraries).
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

from evidence import analyze_direct

sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parent
ROOT = SOURCE.parents[1]
spec = importlib.util.spec_from_file_location("android_smoke", ROOT / "projects/aarch64-limine-crosvm/tools/fixture.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--probes", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path, help="Android /system and /apex runtime tree")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--accel", choices=("tcg", "hvf"), default="tcg")
    parser.add_argument("--binder", action="store_true", help="include two-process Binder RPC checks")
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be finite and positive")
    kernel, probes, output = [p.resolve() for p in (args.kernel, args.probes, args.output)]
    if output.exists():
        parser.error("use a new output directory to preserve earlier evidence")
    for path in [kernel, *(probes / n for n in ["bionic-static", "bionic-dynamic", "libbionic-probe.so", "build.json"])]:
        if not path.is_file():
            parser.error(f"missing input: {path}")
    if args.runtime_root and not args.runtime_root.is_dir():
        parser.error("runtime root must be a directory")
    runtime_files = []
    if args.runtime_root:
        runtime_files = [args.runtime_root / name for name in (
            "system/bin/linker64", "apex/com.android.runtime/lib64/bionic/libc.so",
            "apex/com.android.runtime/lib64/bionic/libdl.so", "apex/com.android.runtime/lib64/bionic/libm.so")]
        for path in runtime_files:
            if not path.is_file():
                parser.error(f"missing Android runtime file: {path}")
    code = smoke.firmware(("SCARLET_EFI_CODE_ARM64_HVF", "SCARLET_EFI_CODE_ARM64")) if args.accel == "hvf" else smoke.firmware(("SCARLET_EFI_CODE_ARM64", "SCARLET_EFI_CODE_ARM64_EL2"))
    variables = smoke.firmware(("SCARLET_EFI_VARS_ARM64_HVF", "SCARLET_EFI_VARS_ARM64")) if args.accel == "hvf" else smoke.firmware(("SCARLET_EFI_VARS_ARM64", "SCARLET_EFI_VARS_ARM64_EL2"))
    output.mkdir(parents=True)
    # The input build may be replaced by another cargo invocation. Bind both
    # image construction and evidence hashes to one immutable run-local copy.
    shutil.copy2(kernel, output / "scarlet.elf")
    kernel = output / "scarlet.elf"
    (output / "inputs.json").write_text(json.dumps({
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "runtime_sha256": {str(p.relative_to(args.runtime_root)): hashlib.sha256(p.read_bytes()).hexdigest()
                           for p in runtime_files},
    }, indent=2) + "\n")
    (output / "result.json").write_text('{"result":"PREPARING"}\n')
    staging = output / "staging"
    android = staging / "systems/android-aarch64"
    boot = output / "boot"
    for tree in (staging, boot):
        for name in ["bin", "dev", "mnt/newroot", "home", "shared", "tmp", "root"]:
            (tree / name).mkdir(parents=True, exist_ok=True)
    for name in ["bin", "lib", "system/bin", "dev", "proc", "sys", "tmp", "root"]:
        (android / name).mkdir(parents=True, exist_ok=True)
    if args.runtime_root:
        shutil.copytree(args.runtime_root, android, dirs_exist_ok=True, symlinks=True)
    for name in ["bionic-static", "bionic-dynamic"]:
        shutil.copy2(probes / name, android / "bin" / name)
    control = staging / "systems/linux-aarch64/bin"
    control.mkdir(parents=True)
    shutil.copy2(probes / "bionic-static", control / "bionic-static")
    shutil.copy2(probes / "libbionic-probe.so", android / "lib/libbionic-probe.so")
    shutil.copy2(probes / "build.json", output / "probes.json")
    env = dict(os.environ, SCARLET_ANDROID_DYNAMIC="1" if args.runtime_root else "0",
               SCARLET_ANDROID_BINDER="1" if args.binder else "0")
    for manifest, binary, destination in [
        (SOURCE / "init/Cargo.toml", "android-direct-init", "init"),
        (ROOT / "user/scarlet-ld/Cargo.toml", "scarlet-ld", "bin/scarlet-ld"),
    ]:
        subprocess.run(["cargo", "build", "--manifest-path", str(manifest), "--release",
                        "--target", "aarch64-unknown-scarlet", "--target-dir", str(output / "target")], env=env, check=True)
        compiled = output / "target/aarch64-unknown-scarlet/release" / binary
        for tree in (staging, boot):
            shutil.copy2(compiled, tree / destination)
    rootfs = output / "rootfs.ext2"
    size = sum(p.stat().st_size for p in staging.rglob("*") if p.is_file() and not p.is_symlink())
    with rootfs.open("xb") as f:
        f.truncate(max(128 * 1024**2, size * 2 + 64 * 1024**2))
    with (output / "rootfs-build.log").open("wb") as log:
        subprocess.run(["mkfs.ext2", "-F", "-d", str(staging), str(rootfs)], stdout=log, stderr=subprocess.STDOUT, check=True)
    archive, image = output / "initramfs.cpio", output / "boot.img"
    smoke.create_initramfs(boot, archive)
    build = ["cargo-scarlet-plugin-limine", "--arch", "aarch64", "--kernel", str(kernel),
             "--initramfs", str(archive), "--output", str(image), "--cache-dir", str(output / "limine-cache"),
             "--cmdline", "console=ttyAMA0 init=/init"]
    with (output / "image-build.log").open("wb") as log:
        subprocess.run(build, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    runtime_vars = output / "efi-vars.fd"
    shutil.copyfile(variables, runtime_vars)
    runtime_vars.chmod(0o600)
    command = ["qemu-system-aarch64", "-machine", "virt,gic-version=3,acpi=off", "-cpu",
               "host" if args.accel == "hvf" else "max", "-m", "2G", "-smp", "1", "-accel", args.accel,
               "-display", "none", "-monitor", "none", "-serial", "stdio", "-no-reboot",
               "-drive", f"if=pflash,format=raw,unit=0,file={smoke.qemu_filename(code)},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={smoke.qemu_filename(runtime_vars)}",
               "-global", "virtio-mmio.force-legacy=false",
               "-drive", f"id=boot,file={smoke.qemu_filename(image)},format=raw,if=none",
               "-device", "virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0",
               "-drive", f"id=root,file={smoke.qemu_filename(rootfs)},format=raw,if=none",
               "-device", "virtio-blk-device,drive=root,bus=virtio-mmio-bus.1",
               "-device", "virtio-rng-device,bus=virtio-mmio-bus.2", "-nic", "none"]
    (output / "commands.json").write_text(json.dumps({"image": build, "qemu": command}, indent=2) + "\n")
    smoke.SUCCESS = re.compile(rb"\nSCARLET_ANDROID_DIRECT_OK\r*\n")
    smoke.FAILURE = re.compile(rb"\nSCARLET_ANDROID_DIRECT_FAIL:")
    result = smoke.run_guest(command, output, args.timeout)
    result["dynamic_runtime"] = bool(args.runtime_root)
    result["probes"] = analyze_direct((output / "serial.log").read_bytes(), dynamic=bool(args.runtime_root), binder=args.binder)
    if result["result"] == "PASS" and result["probes"]["result"] != "PASS":
        result["result"] = "missing or failed child probe evidence"
    (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"Bionic direct execution: {result['result']}")
    return 0 if result["result"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
