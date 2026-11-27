#!/usr/bin/env python3
"""Boot build.sh's private image. Check each native scanout and disconnect head 1."""
import argparse
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "native-desktop-smoke"))
from qmp import request


SIZES = [(800, 600), (960, 540)]
RED, GREEN, BLUE = bytes([221, 43, 91]), bytes([30, 197, 89]), bytes([53, 107, 213])
# None requires a completely black native scanout, including the cursor.
STAGES = {
    "extend": (RED, BLUE),
    "mirror-external": (RED, RED),
    "mirror-repaint": (GREEN, GREEN),
    "disabled-external": (GREEN, None),
    "undo-external": (GREEN, BLUE),
    "migrated-application": (None, (RED, BLUE)),
    "mirror-primary": (BLUE, BLUE),
    "external-only": (None, BLUE),
    "undo-primary": (GREEN, BLUE),
    "unplug-external": (None, BLUE),
    "recovered": (RED,),
    "unplug-extended": (RED, BLUE),
    "kept-workspace": (RED,),
}


def set_monitor(path, width, height):
    """RFB SetDesktopSize feeds QEMU's UI info for this specific virtio scanout."""
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(5)
        sock.connect(str(path))
        with sock.makefile("rb") as stream:
            if stream.readline() != b"RFB 003.008\n":
                raise RuntimeError("Expected QEMU RFB 3.8")
            sock.sendall(b"RFB 003.008\n")
            if 1 not in stream.read(stream.read(1)[0]):
                raise RuntimeError("Expected local unauthenticated VNC socket")
            sock.sendall(b"\x01")
            if stream.read(4) != bytes(4):
                raise RuntimeError("VNC handshake failed")
            sock.sendall(b"\x01")  # shared ClientInit
            init = stream.read(24)
            stream.read(struct.unpack(">I", init[20:])[0])
            screens = int(width > 0)
            packet = struct.pack(">BBHHBB", 251, 0, width, height, screens, 0)
            if screens:
                packet += struct.pack(">IHHHHI", 0, 0, 0, width, height, 0)
            sock.sendall(packet)


def capture(qmp, path, head):
    request(qmp, {"execute": "screendump", "arguments": {
        "device": "gpu", "head": head, "filename": str(path), "format": "ppm"}})


def read_frame(path, head):
    with path.open("rb") as stream:
        assert stream.readline() == b"P6\n", path
        size = tuple(map(int, stream.readline().split()))
        assert size == SIZES[head], (path, size)
        assert stream.readline() == b"255\n", path
        rgb = stream.read()
        assert len(rgb) == 3 * size[0] * size[1], path
        return rgb


def pixel(rgb, size, x, y):
    offset = 3 * (y * size[0] + x)
    return rgb[offset:offset + 3]


def check_mirror(frames, source, filtered=False):
    # Independent landmarks across the complete screen and both black bars.
    target = 1 - source
    sw, sh = SIZES[source]
    tw, th = SIZES[target]
    scale = min(tw / sw, th / sh)
    width, height = round(sw * scale), round(sh * scale)
    left, top = (tw - width) // 2, (th - height) // 2
    for y in range(5, th - 5, 11):
        for x in range(7, tw - 7, 13):
            expected = bytes(3)
            if left <= x < left + width and top <= y < top + height:
                sx, sy = (x - left) * sw // width, (y - top) * sh // height
                expected = pixel(frames[source], SIZES[source], sx, sy)
                # GL filters across texels at edges. Compare uniform source
                # neighborhoods exactly, including both marker and scene.
                if filtered and any(
                    pixel(frames[source], SIZES[source], px, py) != expected
                    for px in range(max(0, sx - 2), min(sw, sx + 3))
                    for py in range(max(0, sy - 2), min(sh, sy + 3))
                ):
                    continue
            actual = pixel(frames[target], SIZES[target], x, y)
            assert actual == expected, (source, target, x, y, actual, expected)


def check_stage(dump, folder, stage, heads):
    deadline = time.monotonic() + 8
    while True:
        try:
            frames = []
            for head in heads:
                color = STAGES[stage][head]
                path = folder / f"{stage}-{head}.ppm"
                dump(path, head)
                frame = read_frame(path, head)
                frames.append(frame)
                if color is None:
                    assert frame == bytes(len(frame)), f"head {head} still lit"
                else:
                    # Require a visible patch from the live SHM window.
                    colors = color if isinstance(color, tuple) else (color,)
                    for visible in colors:
                        assert frame.count(visible * 16) > 30, f"head {head} missing {visible.hex()}"
                    for other in (RED, GREEN, BLUE):
                        if other not in colors:
                            assert other * 16 not in frame, f"head {head} retained {other.hex()}"
            if len(frames) == 2 and stage.startswith("mirror-"):
                check_mirror(frames, int(stage == "mirror-primary"))
            return
        except (AssertionError, subprocess.CalledProcessError) as error:
            if time.monotonic() >= deadline:
                raise RuntimeError(f"{stage}: {error}") from error
            time.sleep(0.1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path, help="boot.img from build.sh")
    parser.add_argument("output", type=Path, help="new directory for serial.log and scanout PPMs")
    parser.add_argument("--accel", choices=["hvf", "tcg"], default="tcg")
    parser.add_argument("--backend", choices=["cpu", "sgfx"], default="cpu")
    args = parser.parse_args()
    if not __debug__:
        parser.error("pixel assertions require Python without -O")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    if args.backend == "sgfx" and sys.platform == "darwin":
        # Cocoa exposes one console at a time. Keep the same console attached
        # for each complete run so changing the viewer cannot cause guest damage.
        with tempfile.TemporaryDirectory(prefix="sws-cocoa-") as tmp:
            helper = str(Path(tmp) / "cocoa")
            # Use Xcode's macOS SDK with its Swift compiler inside the Nix shell.
            env = {"PATH": "/usr/bin:/bin", "HOME": str(Path.home())}
            subprocess.run(["/usr/bin/swiftc", "-O", str(Path(__file__).with_name("cocoa.swift")),
                            "-o", helper], env=env, check=True)
            for head in range(2):
                run_guest(args, output, helper, head)
        for stage in ("mirror-external", "mirror-repaint", "mirror-primary"):
            frames = [read_frame(output / f"{stage}-{i}.ppm", i) for i in range(2)]
            check_mirror(frames, int(stage == "mirror-primary"), filtered=True)
            print(f"{stage}: both VirGL scanouts matched", flush=True)
    else:
        run_guest(args, output)


def run_guest(args, output, helper=None, cocoa_head=None):
    gpu = {"driver": "virtio-gpu-pci", "id": "gpu", "max_outputs": 2,
           "xres": SIZES[0][0], "yres": SIZES[0][1]}
    display = "none"
    expected_stages = list(STAGES)
    success = b"SWS_OUTPUT_ROLES_OK"
    if args.backend == "sgfx":
        gpu.update(driver="virtio-gpu-gl-pci",
                   outputs=[{"name": f"Display {i}", "xres": w, "yres": h}
                            for i, (w, h) in enumerate(SIZES)])
        display = "cocoa,gl=on,retina=off" if sys.platform == "darwin" else "egl-headless"
        expected_stages = expected_stages[:expected_stages.index("unplug-external")]
        success = b"SWS_OUTPUT_ROLES_CONNECTED_OK"
    suffix = "" if cocoa_head is None else f"-{cocoa_head}"
    with tempfile.TemporaryDirectory(prefix="sws-output-") as tmp:
        tmp = Path(tmp)
        qmp, vnc = tmp / "qmp", tmp / "vnc"
        variables = tmp / "vars.fd"
        shutil.copyfile(os.environ["SCARLET_EFI_VARS_ARM64_HVF"], variables)
        cmd = ["qemu-system-aarch64", "-machine", "virt,gic-version=3,acpi=off",
               "-m", "2G", "-smp", "2", "-accel", args.accel,
               "-cpu", "host" if args.accel == "hvf" else "max",
               "-display", display, "-serial", "stdio", "-monitor", "none", "-no-reboot",
               "-drive", f"if=pflash,unit=0,format=raw,readonly=on,file={os.environ['SCARLET_EFI_CODE_ARM64_HVF']}",
               "-drive", f"if=pflash,unit=1,format=raw,file={variables}",
               "-global", "virtio-mmio.force-legacy=false",
               "-drive", f"if=none,id=boot,format=raw,snapshot=on,file={args.image.resolve()}",
               "-device", "virtio-blk-device,drive=boot", "-device", "virtio-keyboard-device",
               "-device", json.dumps(gpu),
               "-qmp", f"unix:{qmp},server=on,wait=off"]
        if args.backend == "cpu":
            cmd += ["-vnc", f"unix:{vnc},display=gpu,head=1"]
        (output / f"command{suffix}.json").write_text(json.dumps(cmd, indent=2) + "\n")
        with (output / f"serial{suffix}.log").open("wb") as log, subprocess.Popen(
            cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        ) as vm:
            def dump(path, head):
                if helper:
                    width, height = SIZES[head]
                    subprocess.run([helper, "capture", str(vm.pid), str(width), str(height),
                                    str(path)], check=True)
                else:
                    capture(qmp, path, head)

            try:
                deadline = time.monotonic() + 120
                while args.backend == "cpu" and not vnc.is_socket():
                    if vm.poll() is not None:
                        raise RuntimeError(vm.stdout.read().decode(errors="replace"))
                    if time.monotonic() >= deadline:
                        raise TimeoutError("VNC startup")
                    time.sleep(0.05)
                if args.backend == "cpu":
                    set_monitor(vnc, *SIZES[1])
                pending = b""
                stages = iter(expected_stages)
                with selectors.DefaultSelector() as selector:
                    selector.register(vm.stdout, selectors.EVENT_READ)
                    while time.monotonic() < deadline:
                        if not selector.select(timeout=1):
                            continue
                        data = os.read(vm.stdout.fileno(), 65536)
                        if not data:
                            raise RuntimeError("Guest exited before completing fixture")
                        log.write(data)
                        log.flush()
                        lines = (pending + data).split(b"\n")
                        pending = lines.pop()
                        for line in lines:
                            if b"panicked at" in line or b"[panic]" in line:
                                raise RuntimeError(line.decode(errors="replace"))
                            if line.strip() == b"SELECT_BACKEND":
                                vm.stdin.write((args.backend + "\n").encode())
                                vm.stdin.flush()
                            if line.startswith(b"OUTPUT_FIXTURE "):
                                if helper and cocoa_head == 1:
                                    subprocess.run([helper, "select", str(vm.pid), str(cocoa_head)], check=True)
                                vm.stdin.write(b"outputs-ready\n")
                                vm.stdin.flush()
                            if line.strip() == success:
                                assert next(stages, None) is None, "guest skipped pixel checks"
                                print(success.decode())
                                return
                            match = re.fullmatch(rb"CHECK_PIXELS ([\w-]+)\r?", line)
                            if match:
                                stage = match[1].decode()
                                assert next(stages) == stage, stage
                                heads = range(len(STAGES[stage])) if cocoa_head is None else [cocoa_head]
                                check_stage(dump, output, stage, heads)
                                source = "QMP" if helper is None else f"Cocoa head {cocoa_head}"
                                print(f"{stage}: {source} pixels passed", flush=True)
                                if stage in ("unplug-external", "unplug-extended"):
                                    set_monitor(vnc, 0, 0)
                                elif stage == "recovered":
                                    set_monitor(vnc, *SIZES[1])
                                vm.stdin.write((stage + "\n").encode())
                                vm.stdin.flush()
                raise TimeoutError(f"Guest did not finish; inspect {log.name}")
            finally:
                vm.terminate()
                try:
                    vm.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    vm.kill()


if __name__ == "__main__":
    main()
