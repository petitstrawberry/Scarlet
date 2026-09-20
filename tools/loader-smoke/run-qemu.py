#!/usr/bin/env python3
"""Boot a native-loader fixture in an isolated AArch64 or RISC-V64 QEMU instance."""

import argparse
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import stat
import subprocess
import sys
import time


SUCCESS = re.compile(rb"\nSCARLET_LOADER_SMOKE_OK\r?\n")
FAILURE = re.compile(rb"\nSCARLET_LOADER_SMOKE_FAIL(?:[ :\r\n]|$)")
INTERPRETER_ERROR = re.compile(rb"\nscarlet-ld: ")
PANIC = re.compile(rb"panicked at|kernel panic|\[panic\]", re.IGNORECASE)


def archive_entry(archive, name, mode, content, inode):
    name_bytes = os.fsencode(name) + b"\0"
    fields = (inode, mode, 0, 0, 1, 0, len(content), 0, 0, 0, 0, len(name_bytes), 0)
    archive.write(b"070701" + b"".join(f"{field:08x}".encode() for field in fields))
    archive.write(name_bytes)
    archive.write(b"\0" * (-(110 + len(name_bytes)) % 4))
    archive.write(content)
    archive.write(b"\0" * (-len(content) % 4))


def create_initramfs(staging, destination):
    """Emit sorted newc entries without host ownership or timestamp variation."""
    with destination.open("wb") as archive:
        paths = [staging, *sorted(staging.rglob("*"))]
        for inode, path in enumerate(paths, 1):
            mode = path.lstat().st_mode
            name = "." if path == staging else path.relative_to(staging).as_posix()
            if stat.S_ISREG(mode):
                content = path.read_bytes()
            elif stat.S_ISLNK(mode):
                content = os.fsencode(os.readlink(path))
            elif stat.S_ISDIR(mode):
                content = b""
            else:
                raise ValueError(f"unsupported staging entry: {path}")
            archive_entry(archive, name, mode, content, inode)
        archive_entry(archive, "TRAILER!!!", 0, b"", len(paths) + 1)
        archive.write(b"\0" * (-archive.tell() % 512))


def firmware(variable_names):
    for name in variable_names:
        value = os.environ.get(name)
        if value and Path(value).is_file():
            return Path(value).resolve()
    raise ValueError("set " + " or ".join(variable_names) + " to an existing firmware file")


def qemu_filename(path):
    return str(path).replace(",", ",,")


def run_guest(command, output, timeout):
    result = "QEMU exited before the success marker"
    started = time.monotonic()
    with (output / "serial.log").open("wb") as serial, subprocess.Popen(
        command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
    ) as process:
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                # Supply the one real start-of-stream boundary; truncation must
                # never invent another beginning-of-line before a partial line.
                tail = b"\n"
                while True:
                    remaining = timeout - (time.monotonic() - started)
                    if remaining <= 0:
                        result = "timeout waiting for the success marker"
                        break
                    events = selector.select(min(remaining, 0.5))
                    if not events:
                        if process.poll() is not None:
                            break
                        continue
                    data = os.read(process.stdout.fileno(), 65536)
                    if not data:
                        break
                    serial.write(data)
                    serial.flush()
                    sys.stdout.buffer.write(data)
                    sys.stdout.buffer.flush()
                    tail += data
                    if PANIC.search(tail):
                        result = "guest panic"
                        break
                    if FAILURE.search(tail):
                        result = "fixture failure marker"
                        break
                    if INTERPRETER_ERROR.search(tail):
                        result = "interpreter error"
                        break
                    if SUCCESS.search(tail):
                        result = "PASS"
                        break
                    tail = tail[-512:]
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        return {"result": result, "qemu_exit_code": process.returncode,
                "elapsed_seconds": round(time.monotonic() - started, 3)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("aarch64", "riscv64"), default="aarch64")
    parser.add_argument("--kernel", type=Path, required=True, help="fresh Limine kernel ELF matching --arch")
    parser.add_argument("--staging", type=Path, required=True, help="fixture root containing /init")
    parser.add_argument("--output", type=Path, required=True, help="dedicated artifacts directory")
    parser.add_argument("--timeout", type=float, default=120, help="guest timeout in seconds (default: 120)")
    parser.add_argument("--limine-cache", type=Path, help="optional existing Limine cache to copy locally")
    parser.add_argument("--prepare-only", action="store_true", help="create boot artifacts without starting QEMU")
    args = parser.parse_args()
    kernel, staging, output = (path.resolve() for path in (args.kernel, args.staging, args.output))
    if not 0 < args.timeout < float("inf"):
        parser.error("--timeout must be positive")
    if output == staging or staging in output.parents:
        parser.error("--output must be outside --staging")
    output.mkdir(parents=True, exist_ok=True)
    # A failed or preparation-only rerun must never leave earlier PASS evidence.
    (output / "result.json").write_text(json.dumps({"result": "PREPARING"}) + "\n")
    (output / "serial.log").unlink(missing_ok=True)
    if not kernel.is_file():
        parser.error(f"kernel does not exist: {kernel}")
    for required in ("init", "system/bin/scarlet-ld"):
        if not (staging / required).is_file():
            parser.error(f"staging is missing {required}")
    qemu = f"qemu-system-{args.arch}"
    for executable in (qemu, "cargo-scarlet-plugin-limine"):
        if shutil.which(executable) is None:
            parser.error(f"{executable} is missing; run inside the Scarlet development shell")
    if args.arch == "aarch64":
        code = firmware(("SCARLET_EFI_CODE_ARM64_EL2", "SCARLET_EFI_CODE_ARM64"))
        variables = firmware(("SCARLET_EFI_VARS_ARM64_EL2", "SCARLET_EFI_VARS_ARM64"))
        machine = ["-machine", "virt,gic-version=3,acpi=off", "-cpu", "max", "-m", "2G"]
        boot_device = "virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0"
        gpu_device = "virtio-gpu-device,bus=virtio-mmio-bus.3"
        console = "ttyAMA0"
    else:
        code = firmware(("SCARLET_EFI_CODE_RV64",))
        variables = firmware(("SCARLET_EFI_VARS_RV64",))
        machine = ["-machine", "virt,acpi=off", "-bios", "default", "-m", "4G"]
        boot_device = "virtio-blk-pci,drive=boot,bus=pcie.0"
        gpu_device = "virtio-gpu-device,bus=virtio-mmio-bus.1"
        console = "ttyS0"
    archive = output / "initramfs.cpio"
    image = output / "boot.img"
    cache = output / "limine-cache"
    if args.limine_cache:
        source_cache = args.limine_cache.resolve()
        if source_cache == cache or source_cache in cache.parents or cache in source_cache.parents:
            parser.error("--limine-cache must not overlap the output cache")
        shutil.copytree(source_cache, cache, dirs_exist_ok=True)
    create_initramfs(staging, archive)
    plugin_command = ["cargo-scarlet-plugin-limine", "--arch", args.arch, "--kernel", str(kernel),
                      "--initramfs", str(archive), "--output", str(image),
                      "--cache-dir", str(cache), "--cmdline", f"console={console} init=/init"]
    with (output / "image-build.log").open("wb") as log:
        subprocess.run(plugin_command, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    runtime_variables = output / "efi-vars.fd"
    shutil.copyfile(variables, runtime_variables)
    runtime_variables.chmod(0o600)
    command = [qemu, *machine, "-accel", "tcg", "-smp", "1", "-no-reboot",
               "-display", "none", "-monitor", "none", "-serial", "stdio",
               "-drive", f"if=pflash,format=raw,unit=0,file={qemu_filename(code)},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={qemu_filename(runtime_variables)}",
               "-global", "virtio-mmio.force-legacy=false",
               "-drive", f"id=boot,file={qemu_filename(image)},format=raw,if=none",
               "-device", boot_device, "-device", gpu_device]
    (output / "commands.json").write_text(json.dumps({"image": plugin_command, "qemu": command}, indent=2) + "\n")
    if args.prepare_only:
        (output / "result.json").write_text(json.dumps({"result": "PREPARED"}) + "\n")
        print(f"Prepared boot artifacts: {output}")
        return 0
    result = run_guest(command, output, args.timeout)
    (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"\nLoader smoke: {result['result']} (artifacts: {output})")
    return 0 if result["result"] == "PASS" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Loader smoke error: {error}", file=sys.stderr)
        sys.exit(1)
