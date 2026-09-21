#!/usr/bin/env python3
"""Require a native linker to create and execute new ELF files inside Scarlet."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "native-rustc"))
spec = importlib.util.spec_from_file_location(
    "native_rustc_qemu", Path(__file__).resolve().parents[1] / "native-rustc/run-qemu.py")
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
smoke = native.smoke


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=native.TARGETS, required=True)
    parser.add_argument("--artifact", type=Path, required=True, help="extracted native-linker directory from Actions")
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--bootstrap", type=Path, required=True, help="standard native static init")
    parser.add_argument("--output", type=Path, required=True, help="new private artifacts directory")
    parser.add_argument("--limine-cache", type=Path)
    parser.add_argument("--timeout", type=float, default=900)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    artifact, kernel, bootstrap, output = (p.resolve() for p in (args.artifact, args.kernel, args.bootstrap, args.output))
    if output.exists() or artifact == output or artifact in output.parents or output in artifact.parents:
        parser.error("output must be a new directory outside the artifact")
    if not 0 < args.timeout < float("inf") or not kernel.is_file():
        parser.error("invalid timeout or kernel")
    manifest = json.loads((artifact / "manifest.json").read_text())
    if manifest.get("target") != native.TARGETS[args.arch] or manifest.get("status") != "built-not-guest-verified":
        parser.error("artifact target/status does not match this test")
    for name, digest in manifest["files"].items():
        path = (artifact / name).resolve()
        if artifact not in path.parents or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            parser.error(f"artifact path or checksum mismatch: {name}")
    native.executable(bootstrap, args.arch, "bootstrap", static=True)
    native.executable(artifact / "bin/wild", args.arch, "native linker", static=True)
    native.executable(artifact / "bin/native-linker-probe", args.arch, "native probe", static=True)
    for tool in (f"qemu-system-{args.arch}", "cargo-scarlet-plugin-limine"):
        if not shutil.which(tool):
            parser.error(f"missing {tool}; enter the Scarlet development shell")
    if args.arch == "aarch64":
        code = smoke.firmware(("SCARLET_EFI_CODE_ARM64_EL2", "SCARLET_EFI_CODE_ARM64"))
        variables = smoke.firmware(("SCARLET_EFI_VARS_ARM64_EL2", "SCARLET_EFI_VARS_ARM64"))
        machine = ["-machine", "virt,gic-version=3,acpi=off", "-cpu", "max"]
        boot_device = "virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0"
        gpu_device = "virtio-gpu-device,bus=virtio-mmio-bus.3"
        console = "ttyAMA0"
    else:
        code = smoke.firmware(("SCARLET_EFI_CODE_RV64",))
        variables = smoke.firmware(("SCARLET_EFI_VARS_RV64",))
        machine = ["-machine", "virt,acpi=off", "-bios", "default"]
        boot_device = "virtio-blk-pci,drive=boot,bus=pcie.0"
        gpu_device = "virtio-gpu-device,bus=virtio-mmio-bus.1"
        console = "ttyS0"
    output.mkdir(parents=True)
    result_path = output / "result.json"
    result_path.write_text(json.dumps({"result": "PREPARING", "guest_link_verified": False}) + "\n")
    (output / "artifact-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    staging = output / "root-tree"
    for directory in ("system/bin", "opt/native-linker/fixtures", "tmp", "dev", "mnt/newroot", "root"):
        (staging / directory).mkdir(parents=True, exist_ok=True)
    shutil.copy2(bootstrap, staging / "init")
    for name in ("wild", "native-linker-probe"):
        shutil.copy2(artifact / "bin" / name, staging / "system/bin" / name)
    # Stage only link inputs. Pre-linked host test executables cannot satisfy the probe.
    for name in ("main.o", "answer.o", "bias.o", "libanswer.a"):
        shutil.copy2(artifact / "fixtures" / name, staging / "opt/native-linker/fixtures" / name)
    if sum(p.stat().st_size for p in staging.rglob("*") if p.is_file()) >= 256 * 1048576:
        raise ValueError("linker initramfs exceeds the 256 MiB bring-up limit")
    archive, boot_image, cache = output / "initramfs.cpio", output / "boot.img", output / "limine-cache"
    if args.limine_cache:
        source_cache = args.limine_cache.resolve()
        if source_cache == output or source_cache in output.parents or output in source_cache.parents:
            raise ValueError("Limine cache overlaps output")
        shutil.copytree(source_cache, cache)
    smoke.create_initramfs(staging, archive)
    cmdline = f"console={console} init=/init init.exec=/system/bin/native-linker-probe init.console=/dev/tty0"
    image = ["cargo-scarlet-plugin-limine", "--arch", args.arch, "--kernel", str(kernel),
             "--initramfs", str(archive), "--output", str(boot_image), "--cache-dir", str(cache), "--cmdline", cmdline]
    with (output / "image-build.log").open("wb") as log:
        subprocess.run(image, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    runtime_variables = output / "efi-vars.fd"
    shutil.copyfile(variables, runtime_variables)
    runtime_variables.chmod(0o600)
    command = [f"qemu-system-{args.arch}", *machine, "-m", "4G", "-accel", "tcg", "-smp", "1",
               "-no-reboot", "-display", "none", "-monitor", "none", "-serial", "stdio",
               "-drive", f"if=pflash,format=raw,unit=0,file={smoke.qemu_filename(code)},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={smoke.qemu_filename(runtime_variables)}",
               "-global", "virtio-mmio.force-legacy=false",
               "-drive", f"id=boot,file={smoke.qemu_filename(boot_image)},format=raw,if=none", "-device", boot_device,
               "-device", gpu_device, *native.entropy_arguments(args.arch)]
    (output / "commands.json").write_text(json.dumps({"image": image, "qemu": command}, indent=2) + "\n")
    if args.prepare_only:
        result_path.write_text(json.dumps({"result": "PREPARED", "guest_link_verified": False}) + "\n")
        return 0
    smoke.SUCCESS = re.compile(rb"\nNATIVE_LINKER FULL PASS\r?\n")
    smoke.FAILURE = re.compile(rb"\nNATIVE_LINKER FAIL(?:[ :\r\n]|$)")
    result = smoke.run_guest(command, output, args.timeout)
    result.update(guest_link_verified=result["result"] == "PASS", architecture=args.arch)
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"\nNative linker: {result['result']} (evidence: {output})")
    return 0 if result["guest_link_verified"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Native linker harness: {error}", file=sys.stderr)
        sys.exit(1)
