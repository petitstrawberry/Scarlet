#!/usr/bin/env python3
"""Boot Scarlet with a test-only H4 radio, then exercise btctl, joycond and SWS navigation."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.dont_write_bytecode = True


def build(out):
    def cargo(manifest, args, target_dir, log, env=None):
        cmd = ["cargo", "build", "--locked", "--release", "--manifest-path", str(manifest),
               "--target-dir", str(target_dir), *args]
        with (out / log).open("w") as destination:
            result = subprocess.run(cmd, cwd=ROOT, env=env, stdout=destination, stderr=subprocess.STDOUT)
        if result.returncode:
            lines = (out / log).read_text().splitlines()
            first = next((i for i, line in enumerate(lines) if line.startswith("error")), None)
            excerpt = lines[-30:] if first is None else lines[first:first + 30]
            raise RuntimeError(f"Build failed ({log}):\n" + "\n".join(excerpt))

    spec = json.loads((ROOT / "kernel/targets/aarch64-unknown-none-elf.json").read_text())
    spec.pop("pre-link-args")
    target = out / "bt-none.json"
    target.write_text(json.dumps(spec))
    env = os.environ.copy()
    env["CARGO_UNSTABLE_JSON_TARGET_SPEC"] = "true"
    env["RUSTFLAGS"] = (f"-C link-arg=-T{ROOT}/tools/network-management/guest/kernel.ld "
                        "-C no-vectorize-loops -C no-vectorize-slp")
    cargo(HERE / "fixture/Cargo.toml", ["--target", str(target), "-Z",
          "build-std=core,alloc,compiler_builtins", "-Z", "build-std-features=compiler-builtins-mem"],
          out / "kernel", "kernel.log", env)
    subprocess.run(["llvm-objcopy", "--remove-section=.ksym", "-O", "binary",
                    out / "kernel/bt-none/release/bluetooth-qa-kernel", out / "Image"], check=True)
    for manifest, args, log in [
        (ROOT / "user/bin/Cargo.toml", ["--no-default-features", "--bin", "init"], "init.log"),
        (ROOT / "user/scarlet-ld/Cargo.toml", [], "loader.log"),
        (ROOT / "user/bluetooth/Cargo.toml", [], "btctl.log"),
        (HERE / "guest/Cargo.toml", [], "guest.log"),
    ]:
        cargo(manifest, ["--target", "aarch64-unknown-scarlet", *args], out / "user", log)


def boot(out):
    module = importlib.util.spec_from_file_location("loader_smoke", ROOT / "tests/loader-smoke/run-qemu.py")
    smoke = importlib.util.module_from_spec(module)
    module.loader.exec_module(smoke)
    stage = out / "stage"
    for name in ["bin", "dev", "etc", "mnt/newroot", "root", "tmp"]:
        (stage / name).mkdir(mode=0o755, parents=True, exist_ok=True)
    for name in ["init", "bin/scarlet-ld", "bin/btctl", "bin/joycond", "bin/bluetooth-qa"]:
        binary = out / "user/aarch64-unknown-scarlet/release" / Path(name).name
        (stage / name).write_bytes(binary.read_bytes())
        (stage / name).chmod(0o755)
    smoke.create_initramfs(stage, out / "initramfs.cpio")
    # btctl verifies the inode permissions protecting /etc and its bond files.
    # Use a private ext2 root as on the console. Archive-only cpio metadata
    # cannot describe the ownership/ACL information required for pairing.
    disk = out / "root.ext2"
    with disk.open("wb") as stream:
        stream.truncate(48 * 1024 * 1024)
    subprocess.run(["mke2fs", "-q", "-F", "-t", "ext2", "-b", "4096",
                    "-E", "root_owner=0:0", "-d", str(stage), str(disk)], check=True)
    for field in ["uid", "gid"]:
        subprocess.run(["debugfs", "-w", "-R", f"set_inode_field /etc {field} 0", str(disk)],
                       check=True, stdout=subprocess.DEVNULL)
    cmd = ["qemu-system-aarch64", "-machine", "virt,gic-version=3", "-cpu", "max", "-m", "2G",
           "-smp", "2", "-nic", "none", "-display", "none", "-monitor", "none", "-serial", "stdio",
           "-no-reboot", "-kernel", str(out / "Image"), "-initrd", str(out / "initramfs.cpio"),
           "-global", "virtio-mmio.force-legacy=false",
           "-drive", f"id=rootfs,file={disk},if=none,format=raw",
           "-device", "virtio-blk-device,drive=rootfs",
           "-append", "console=ttyAMA0 root=/dev/vblk0 rootwait rootfstype=ext2 init.exec=/bin/bluetooth-qa"]
    log = out / "serial.log"
    with log.open("wb") as serial, subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=serial,
                                                    stderr=subprocess.STDOUT) as qemu:
        try:
            deadline = time.monotonic() + 180
            reported = 0
            while time.monotonic() < deadline:
                output = log.read_text(errors="replace")
                lines = output.splitlines()
                complete = len(lines) if output.endswith("\n") else max(0, len(lines) - 1)
                for line in lines[reported:complete]:
                    if line.startswith(("BT_QA_", "BLUETOOTH_")):
                        print(line, flush=True)
                    if line == "BLUETOOTH_QEMU_PASS":
                        return
                reported = complete
                if qemu.poll() is not None or any("panicked at" in line or "[panic]" in line for line in lines):
                    break
                time.sleep(0.1)
            raise RuntimeError("Bluetooth QEMU failed:\n" + "\n".join(lines[-60:]))
        finally:
            if qemu.poll() is None:
                qemu.terminate()
                try:
                    qemu.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    qemu.kill()
                    qemu.wait()


def run(out, skip_build):
    out.mkdir(exist_ok=True, parents=True)
    if not skip_build:
        build(out)
    boot(out)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="retain images and serial.log in a dedicated scratch directory")
    parser.add_argument("--no-build", action="store_true", help="reuse this test's previously built artifacts")
    args = parser.parse_args()
    if args.no_build and not args.output:
        parser.error("--no-build requires --output")
    if args.output:
        run(args.output.resolve(), args.no_build)
    else:
        with tempfile.TemporaryDirectory(prefix="scarlet-bluetooth-qa-") as scratch:
            run(Path(scratch), False)
