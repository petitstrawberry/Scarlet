#!/usr/bin/env python3
"""Boot a bounded, isolated native desktop without building the full rootfs."""
import argparse
import fcntl
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import time
from qmp import request

REPO = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["kernel", "staging", "output", "limine-plugin", "qemu", "firmware-code", "firmware-vars"]:
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--duration", type=int, default=300, help="total runtime, 15–900 seconds")
    parser.add_argument("--screenshot-after", type=int, default=60)
    args = parser.parse_args()
    if not 15 <= args.duration <= 900 or not 0 < args.screenshot_after < args.duration:
        parser.error("duration must be 15–900 seconds and screenshot must precede its end")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    # Lock before touching an old boot image or QMP socket. A concurrent run
    # must never replace files that another guest still has mounted.
    lock = (output / "run.lock").open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        parser.error(f"Another smoke run owns {output}; choose a different output directory")
    spec = importlib.util.spec_from_file_location("loader_smoke", REPO / "tests/loader-smoke/run-qemu.py")
    smoke = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(smoke)
    archive, image = output / "initramfs.cpio", output / "boot.img"
    smoke.create_initramfs(args.staging.resolve(), archive)
    image_command = [str(args.limine_plugin.resolve()), "--arch", "aarch64", "--kernel", str(args.kernel.resolve()),
                     "--initramfs", str(archive), "--output", str(image), "--cache-dir", str(output / "limine-cache"),
                     "--cmdline", "console=ttyAMA0 init=/init init.console=/dev/tty0 init.exec=/bin/prepare-desktop"]
    with (output / "image-build.log").open("wb") as log:
        subprocess.run(image_command, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    variables = output / "efi-vars.fd"
    shutil.copyfile(args.firmware_vars, variables)
    variables.chmod(0o600)
    control = output / "control.sock"
    control.unlink(missing_ok=True)
    command = [str(args.qemu.resolve()), "-machine", "virt,gic-version=3,acpi=off", "-cpu", "max",
               "-m", "2G", "-accel", "tcg", "-smp", "2", "-no-reboot", "-display", "none",
               "-monitor", "none", "-serial", "stdio",
               "-drive", f"if=pflash,format=raw,unit=0,file={args.firmware_code.resolve()},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={variables}",
               "-global", "virtio-mmio.force-legacy=false", "-drive", f"id=boot,file={image},format=raw,if=none",
               "-device", "virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0",
               "-device", "virtio-keyboard-device,bus=virtio-mmio-bus.1",
               "-device", "virtio-tablet-device,bus=virtio-mmio-bus.2",
               "-device", "virtio-gpu-device,bus=virtio-mmio-bus.3",
               "-qmp", f"unix:{control},server=on,wait=off"]
    manifest = {"image": image_command, "qemu": command,
                "kernel_sha256": hashlib.sha256(args.kernel.read_bytes()).hexdigest(),
                "initramfs_sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                "duration_seconds": args.duration}
    (output / "commands.json").write_text(json.dumps(manifest, indent=2) + "\n")
    started = time.monotonic()
    with (output / "serial.log").open("wb") as serial, subprocess.Popen(
        command, stdin=subprocess.DEVNULL, stdout=serial, stderr=subprocess.STDOUT
    ) as process:
        print(f"QEMU_PID={process.pid} QMP={control}", flush=True)
        captured = False
        try:
            while time.monotonic() - started < args.duration:
                if process.poll() is not None:
                    raise RuntimeError(f"QEMU exited early: {process.returncode}; inspect serial.log")
                if not captured and time.monotonic() - started >= args.screenshot_after:
                    request(control, {"execute": "screendump", "arguments": {
                        "filename": str(output / "initial.png"), "format": "png"}})
                    captured = True
                    print(f"SCREENSHOT={output / 'initial.png'}", flush=True)
                time.sleep(0.25)
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            (output / "run-result.json").write_text(json.dumps({
                "elapsed_seconds": round(time.monotonic() - started, 3), "screenshot_captured": captured,
                "exit_code": process.returncode, "note": "A screenshot is evidence to inspect, not an automatic functional pass."
            }, indent=2) + "\n")


if __name__ == "__main__":
    main()
