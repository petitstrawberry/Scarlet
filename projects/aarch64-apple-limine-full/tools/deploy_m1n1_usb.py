#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build and boot Scarlet on Apple Silicon through m1n1 USB proxy mode."""

import argparse
import os
import pathlib
import platform
import subprocess
import sys
import threading
import time

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
PROJECT_DIR = pathlib.Path(__file__).resolve().parents[1]
M1N1_DIR = PROJECT_DIR / "m1n1"
PROXYCLIENT_DIR = M1N1_DIR / "proxyclient"
TOOLS_DIR = PROXYCLIENT_DIR / "tools"
DEFAULT_IMAGE_ADDR = 0x900000000
DEFAULT_IMAGE_MAP_SIZE = 0x10000000
DEFAULT_ENTRY_POINT = 0x800

sys.path.append(str(PROXYCLIENT_DIR))


def detect_devices():
    import glob as _glob
    if platform.system() == "Darwin":
        pattern = "/dev/cu.usbmodem*"
    else:
        pattern = "/dev/ttyACM*"
    matches = sorted(_glob.glob(pattern))
    if len(matches) >= 2:
        return matches[0], matches[1]
    return None, None


def wait_for_devices(timeout):
    if os.environ.get("M1N1DEVICE"):
        proxy = os.environ["M1N1DEVICE"]
        import pathlib as _p
        stem = _p.Path(proxy).stem
        sec = str(_p.Path(proxy).with_name(stem[:-1] + "3")) if stem[-1] == "1" else proxy
        wait_for_device(proxy, timeout)
        return proxy, sec

    print("Waiting for m1n1 USB devices to appear (Ctrl+C to abort)...")
    start = time.monotonic()
    while True:
        proxy, secondary = detect_devices()
        if proxy:
            print(f"  Proxy:     {proxy}")
            print(f"  Secondary: {secondary}")
            return proxy, secondary
        if timeout is not None and time.monotonic() - start > timeout:
            raise TimeoutError("Timed out waiting for m1n1 USB devices")
        elapsed = int(time.monotonic() - start)
        sys.stdout.write(f"\r  Scanning... ({elapsed}s)  ")
        sys.stdout.flush()
        time.sleep(0.25)


def parse_int(value):
    return int(value, 0)


def wait_for_device(path, timeout):
    if not path:
        return
    print(f"Waiting for device file '{path}' to appear (Ctrl+C to abort)...")
    start = time.monotonic()
    while not pathlib.Path(path).exists():
        if timeout is not None and time.monotonic() - start > timeout:
            raise TimeoutError(f"Timed out waiting for {path}")
        time.sleep(0.25)


def run_checked(cmd, *, cwd=REPO_ROOT, env=None):
    print("+ " + " ".join(str(part) for part in cmd))
    subprocess.run([str(part) for part in cmd], cwd=cwd, env=env, check=True)


class UartRouter:
    def __init__(self, device, logfile=None, baudrate=500000):
        self.device = device
        self.logfile = logfile
        self.baudrate = baudrate
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._run, name="scarlet-uart-router", daemon=True)

    def start(self):
        self.thread.start()

    def close(self):
        self.stop.set()

    def _run(self):
        import serial

        log = self.logfile.open("ab") if self.logfile else None
        try:
            while not self.stop.is_set():
                if not self.device.exists():
                    time.sleep(0.25)
                    continue
                try:
                    with serial.Serial(str(self.device), self.baudrate, timeout=0.25) as ser:
                        ser.reset_input_buffer()
                        print(f"UART capture connected to {self.device} at {self.baudrate} baud")
                        while not self.stop.is_set():
                            data = ser.read(4096)
                            if not data:
                                continue
                            if log:
                                log.write(data)
                                log.flush()
                            sys.stdout.buffer.write(data)
                            sys.stdout.buffer.flush()
                except (OSError, serial.SerialException) as exc:
                    print(f"UART capture disconnected: {exc}", file=sys.stderr)
                    time.sleep(1)
        finally:
            if log:
                log.close()


def build_image(args):
    cmd = ["cargo", "scarlet", "image", "--project", args.project]
    if args.release:
        cmd.append("--release")
    run_checked(cmd)


def chainload(args):
    env = os.environ.copy()
    env["M1N1DEVICE"] = args.proxy_device
    run_checked([sys.executable, TOOLS_DIR / "chainload.py", "-r", args.m1n1], cwd=REPO_ROOT, env=env)


def start_guest(args):
    from m1n1.proxy import UartInterface, M1N1Proxy
    from m1n1.proxyutils import ProxyUtils, bootstrap_port
    from m1n1.hv import HV
    from m1n1.hw.pmu import PMU

    wait_for_device(args.proxy_device, 30)

    iface = UartInterface(device=args.proxy_device)
    p = M1N1Proxy(iface, debug=False)
    bootstrap_port(iface, p)
    u = ProxyUtils(p, heap_size=128 * 1024 * 1024)

    hv = HV(iface, p, u)
    iface.dev.reset_input_buffer()
    hv.init()

    payload = args.payload.read_bytes()
    print(f"Loading guest payload {args.payload} ({len(payload)} bytes)")
    hv.load_raw(payload, args.entry_point)

    image = args.image.read_bytes()
    if len(image) > args.image_map_size:
        raise ValueError(
            f"Scarlet image is {len(image)} bytes, larger than U-Boot blkmap window "
            f"0x{args.image_map_size:x} bytes"
        )

    mem_top = u.ba.phys_base + u.ba.mem_size
    if args.image_addr < hv.ram_base or args.image_addr + len(image) > mem_top:
        raise ValueError(
            f"Image load range 0x{args.image_addr:x}..0x{args.image_addr + len(image):x} "
            f"is outside m1n1 guest RAM 0x{hv.ram_base:x}..0x{mem_top:x}"
        )

    print(
        f"Pushing Scarlet Limine image {args.image} ({len(image)} bytes) "
        f"to 0x{args.image_addr:x}"
    )
    iface.writemem(args.image_addr, image, True)
    p.dc_cvau(args.image_addr, len(image))

    PMU(u).reset_panic_counter()
    print("Starting guest under m1n1 hypervisor")
    hv.start()


def existing_path(path):
    path = pathlib.Path(path)
    if not path.exists():
        raise argparse.ArgumentTypeError(f"{path} does not exist")
    return path


def main():
    parser = argparse.ArgumentParser(
        description="Build Scarlet, push its Limine UEFI image to Apple Silicon RAM, and boot it via m1n1 HV."
    )
    parser.add_argument("--project", default=str(PROJECT_DIR), help="Scarlet project directory")
    parser.add_argument("--release", action="store_true", help="Build the Scarlet image in release mode")
    parser.add_argument("--no-build", action="store_true", help="Use the existing Scarlet image")
    parser.add_argument("--skip-chainload", action="store_true", help="Do not chainload fresh m1n1 before starting HV")
    parser.add_argument("--proxy-device", default=None, help="Primary m1n1 proxy UART device (auto-detected if omitted)")
    parser.add_argument("--secondary-device", default=None, help="Secondary HV virtual UART device (auto-detected if omitted)")
    parser.add_argument("--no-uart", action="store_true", help="Do not capture secondary UART output")
    parser.add_argument("--uart-log", type=pathlib.Path, help="Optional file to append secondary UART output")
    parser.add_argument("--connect-timeout", type=float, default=None, help="Seconds to wait for USB device files")
    parser.add_argument("--m1n1", type=existing_path, default=M1N1_DIR / "payloads" / "m1n1.bin", help="Fresh raw m1n1.bin to chainload")
    parser.add_argument("--payload", type=existing_path, default=M1N1_DIR / "payloads" / "boot-j293.bin", help="Guest raw payload: m1n1 + DTB + U-Boot")
    parser.add_argument("--machine", default="j293", help="Machine code for DTB selection")
    parser.add_argument("--image", type=pathlib.Path, default=PROJECT_DIR / ".scarlet" / "images" / "limine-aarch64-apple-full.img", help="Scarlet Limine UEFI image")
    parser.add_argument("--image-addr", type=parse_int, default=DEFAULT_IMAGE_ADDR, help="Guest physical RAM address used by U-Boot blkmap")
    parser.add_argument("--image-map-size", type=parse_int, default=DEFAULT_IMAGE_MAP_SIZE, help="U-Boot blkmap window size")
    parser.add_argument("--entry-point", type=parse_int, default=DEFAULT_ENTRY_POINT, help="Raw guest payload entry offset")
    args = parser.parse_args()

    args.project = str(pathlib.Path(args.project).resolve())
    args.image = pathlib.Path(args.image).resolve()
    args.payload = pathlib.Path(args.payload).resolve()
    args.m1n1 = pathlib.Path(args.m1n1).resolve()

    if not args.proxy_device or not args.secondary_device:
        args.proxy_device, args.secondary_device = wait_for_devices(args.connect_timeout)
    args.secondary_device = pathlib.Path(args.secondary_device)

    uart = None
    try:
        if not args.no_uart:
            uart = UartRouter(args.secondary_device, args.uart_log)
            uart.start()

        if not args.no_build:
            build_image(args)
        if not args.image.exists():
            raise FileNotFoundError(f"Scarlet image not found: {args.image}")

        if not args.skip_chainload:
            chainload(args)

        start_guest(args)
    finally:
        if uart:
            uart.close()


if __name__ == "__main__":
    main()
