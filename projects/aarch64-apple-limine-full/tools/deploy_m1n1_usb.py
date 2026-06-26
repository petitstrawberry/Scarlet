#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build and boot Scarlet on Apple Silicon through m1n1 USB proxy mode."""

import argparse
import os
import pathlib
import platform
import shlex
import shutil
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


def run_checked_capture(cmd, *, cwd=REPO_ROOT, env=None, echo=True):
    if echo:
        print("+ " + " ".join(str(part) for part in cmd))
    return subprocess.run(
        [str(part) for part in cmd],
        cwd=cwd,
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def timeout_deadline(timeout):
    if timeout is None:
        return None
    return time.monotonic() + timeout


def timeout_remaining(deadline):
    if deadline is None:
        return None
    return max(0.0, deadline - time.monotonic())


def retry_sleep(deadline, interval=1.0):
    remaining = timeout_remaining(deadline)
    if remaining is not None and remaining <= 0:
        return
    time.sleep(interval if remaining is None else min(interval, remaining))


def summarize_failure(exc):
    for text in (getattr(exc, "stderr", None), getattr(exc, "output", None), str(exc)):
        if not text:
            continue
        lines = [line.strip() for line in str(text).splitlines() if line.strip()]
        if lines:
            return lines[-1]
    return type(exc).__name__


def env_flag(name):
    value = os.environ.get(name)
    if value is None:
        return None
    return value.lower() in ("1", "true", "yes", "on")


def shell_join(cmd):
    return shlex.join(str(part) for part in cmd)


def serial_device_busy(device):
    lsof = shutil.which("lsof")
    if not lsof:
        return False
    result = subprocess.run(
        [lsof, str(device)],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        check=False,
    )
    return result.returncode == 0 and bool(result.stdout.strip())


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
        self.thread.join(timeout=2)

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
    cmd = [sys.executable, TOOLS_DIR / "chainload.py", "-r", args.m1n1]
    deadline = timeout_deadline(args.connect_timeout)
    attempt = 1
    print("+ " + " ".join(str(part) for part in cmd))

    while True:
        try:
            result = run_checked_capture(cmd, cwd=REPO_ROOT, env=env, echo=False)
            if result.stdout:
                print(result.stdout, end="")
            if result.stderr:
                print(result.stderr, end="", file=sys.stderr)
            return
        except subprocess.CalledProcessError as exc:
            remaining = timeout_remaining(deadline)
            if remaining is not None and remaining <= 0:
                raise
            suffix = "" if remaining is None else f" ({remaining:.0f}s left)"
            print(
                f"m1n1 chainload not ready on attempt {attempt}: "
                f"{summarize_failure(exc)}; retrying{suffix}",
                file=sys.stderr,
            )
            attempt += 1
            retry_sleep(deadline)


def runner_command_args(args):
    cmd = [
        sys.executable,
        pathlib.Path(__file__).resolve(),
        "--project",
        args.project,
        "--proxy-device",
        args.proxy_device,
        "--secondary-device",
        str(args.secondary_device),
        "--m1n1",
        args.m1n1,
        "--payload",
        args.payload,
        "--machine",
        args.machine,
        "--image",
        args.image,
        "--image-addr",
        hex(args.image_addr),
        "--image-map-size",
        hex(args.image_map_size),
        "--entry-point",
        hex(args.entry_point),
        "--uart-baudrate",
        str(args.uart_baudrate),
        "--no-tmux",
        "--no-uart",
    ]
    if args.release:
        cmd.append("--release")
    if args.no_build:
        cmd.append("--no-build")
    if args.skip_chainload:
        cmd.append("--skip-chainload")
    if args.connect_timeout is not None:
        cmd.extend(["--connect-timeout", str(args.connect_timeout)])
    return cmd


def picocom_command_args(args):
    picocom = shutil.which(args.picocom) or args.picocom
    cmd = [
        picocom,
        "--omap",
        "crlf",
        "--imap",
        "lfcrlf",
        "--baud",
        str(args.uart_baudrate),
    ]
    if args.uart_log:
        cmd.extend(["--logfile", args.uart_log])
    cmd.append(str(args.secondary_device))
    return cmd


def uart_console_command_args(args):
    cmd = [
        sys.executable,
        pathlib.Path(__file__).resolve(),
        "--uart-console-only",
        "--secondary-device",
        str(args.secondary_device),
        "--uart-baudrate",
        str(args.uart_baudrate),
        "--picocom",
        args.picocom,
        "--no-tmux",
    ]
    if args.uart_log:
        cmd.extend(["--uart-log", args.uart_log])
    if args.connect_timeout is not None:
        cmd.extend(["--connect-timeout", str(args.connect_timeout)])
    return cmd


def run_uart_console(args):
    if not args.secondary_device:
        _, args.secondary_device = wait_for_devices(args.connect_timeout)
    args.secondary_device = pathlib.Path(args.secondary_device)

    if not shutil.which(args.picocom):
        raise FileNotFoundError(f"picocom not found: {args.picocom}")

    first_connect = True
    print(f"UART console on {args.secondary_device}")
    print("picocom restarts across USB reconnects. Press Ctrl-C in this pane to stop.")

    while True:
        try:
            wait_for_device(
                args.secondary_device,
                args.connect_timeout if first_connect else None,
            )
            cmd = picocom_command_args(args)
            print("+ " + " ".join(str(part) for part in cmd))
            status = subprocess.run([str(part) for part in cmd], check=False).returncode
            first_connect = False
            print(f"picocom exited with status {status}; reconnecting in 1s")
            time.sleep(1)
        except KeyboardInterrupt:
            print("\nUART console stopped")
            return


def tmux_session_exists(tmux, session):
    return (
        subprocess.run(
            [tmux, "has-session", "-t", session],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode
        == 0
    )


def unique_tmux_session(tmux, requested):
    if not tmux_session_exists(tmux, requested):
        return requested
    index = 1
    while True:
        candidate = f"{requested}-{index}"
        if not tmux_session_exists(tmux, candidate):
            print(f"tmux session '{requested}' already exists; using '{candidate}'")
            return candidate
        index += 1


def ensure_usb_devices(args):
    if not args.proxy_device or not args.secondary_device:
        args.proxy_device, args.secondary_device = wait_for_devices(args.connect_timeout)
    args.secondary_device = pathlib.Path(args.secondary_device)


def launch_tmux(args):
    tmux = shutil.which("tmux")
    if not tmux:
        raise FileNotFoundError("tmux not found")
    if not args.no_uart and not shutil.which(args.picocom):
        raise FileNotFoundError(f"picocom not found: {args.picocom}")

    ensure_usb_devices(args)

    session = unique_tmux_session(tmux, args.tmux_session)
    runner = shell_join(["env", "SCARLET_M1N1_IN_TMUX=1", *runner_command_args(args)])
    if not args.tmux_keep_on_exit:
        runner = (
            f"{runner}; status=$?; "
            'printf "\\nHV runner exited with status %s. Press Enter to close tmux session..." "$status"; '
            "read _; "
            f"tmux kill-session -t {shlex.quote(session)}; "
            'exit "$status"'
        )

    run_checked([tmux, "new-session", "-d", "-s", session, "-n", "hv", runner], cwd=PROJECT_DIR)

    if not args.no_uart:
        if serial_device_busy(args.secondary_device):
            print(f"Secondary UART {args.secondary_device} is already open; not starting picocom")
        else:
            uart = shell_join(["env", "SCARLET_M1N1_IN_TMUX=1", *uart_console_command_args(args)])
            run_checked([tmux, "split-window", "-h", "-t", f"{session}:0", uart], cwd=PROJECT_DIR)
            run_checked([tmux, "select-layout", "-t", f"{session}:0", "even-horizontal"], cwd=PROJECT_DIR)

    run_checked([tmux, "select-pane", "-t", f"{session}:0.0"], cwd=PROJECT_DIR)
    if os.environ.get("TMUX"):
        run_checked([tmux, "switch-client", "-t", session], cwd=PROJECT_DIR)
    else:
        subprocess.run([tmux, "attach-session", "-t", session], cwd=PROJECT_DIR, check=False)


def should_launch_tmux(args):
    if args.no_tmux or os.environ.get("SCARLET_M1N1_IN_TMUX"):
        return False
    env = env_flag("SCARLET_M1N1_TMUX")
    if env is not None:
        return env
    if args.tmux:
        return True
    return (
        not args.no_uart
        and sys.stdin.isatty()
        and sys.stdout.isatty()
        and shutil.which("tmux") is not None
        and shutil.which(args.picocom) is not None
    )


def open_proxy(args, *, timeout):
    from m1n1.proxy import UartInterface, M1N1Proxy, UartTimeout
    from m1n1.proxyutils import bootstrap_port
    import serial

    deadline = timeout_deadline(timeout)
    attempt = 1

    while True:
        wait_for_device(args.proxy_device, timeout_remaining(deadline))
        iface = None
        try:
            iface = UartInterface(device=args.proxy_device)
            p = M1N1Proxy(iface, debug=False)
            bootstrap_port(iface, p)
            return iface, p
        except (OSError, serial.SerialException, UartTimeout) as exc:
            if iface is not None:
                try:
                    iface.dev.close()
                except Exception:
                    pass
            remaining = timeout_remaining(deadline)
            if remaining is not None and remaining <= 0:
                raise
            suffix = "" if remaining is None else f" ({remaining:.0f}s left)"
            print(
                f"m1n1 proxy not ready on attempt {attempt}: "
                f"{summarize_failure(exc)}; retrying{suffix}",
                file=sys.stderr,
            )
            attempt += 1
            retry_sleep(deadline)


def start_guest(args):
    from m1n1.proxyutils import ProxyUtils
    from m1n1.hv import HV
    from m1n1.hw.pmu import PMU

    iface, p = open_proxy(args, timeout=args.connect_timeout)
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
    parser.add_argument("--uart-console-only", action="store_true", help="Open only the reconnecting secondary UART picocom console")
    parser.add_argument("--uart-baudrate", type=int, default=500000, help="Secondary UART baud rate")
    parser.add_argument("--uart-log", type=pathlib.Path, help="Optional file to append secondary UART output")
    parser.add_argument("--tmux", action="store_true", help="Run HV control and UART console in split tmux panes")
    parser.add_argument("--no-tmux", action="store_true", help="Disable automatic tmux split mode")
    parser.add_argument("--tmux-session", default=os.environ.get("SCARLET_M1N1_TMUX_SESSION", "scarlet-m1n1"), help="tmux session name")
    parser.add_argument("--tmux-keep-on-exit", action="store_true", help="Keep tmux panes open after the HV runner exits")
    parser.add_argument("--picocom", default=os.environ.get("SCARLET_M1N1_PICOCOM", "picocom"), help="picocom executable for tmux UART console")
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

    if args.uart_console_only:
        try:
            run_uart_console(args)
            return
        except FileNotFoundError as exc:
            print(f"UART console unavailable: {exc}", file=sys.stderr)
            sys.exit(1)
        except TimeoutError as exc:
            print(f"timeout: {exc}", file=sys.stderr)
            sys.exit(1)

    if should_launch_tmux(args):
        try:
            launch_tmux(args)
            return
        except FileNotFoundError as exc:
            if args.tmux or env_flag("SCARLET_M1N1_TMUX"):
                print(f"tmux mode unavailable: {exc}", file=sys.stderr)
                sys.exit(1)
            print(f"tmux mode unavailable: {exc}; falling back to inline UART capture", file=sys.stderr)

    ensure_usb_devices(args)

    uart = None
    try:
        if not args.no_uart:
            uart = UartRouter(args.secondary_device, args.uart_log, args.uart_baudrate)
            uart.start()

        if not args.no_build:
            build_image(args)
        if not args.image.exists():
            raise FileNotFoundError(f"Scarlet image not found: {args.image}")

        if not args.skip_chainload:
            chainload(args)

        start_guest(args)
    except subprocess.CalledProcessError as exc:
        print(
            f"command failed with exit status {exc.returncode}: "
            + " ".join(str(part) for part in exc.cmd),
            file=sys.stderr,
        )
        print(f"last error: {summarize_failure(exc)}", file=sys.stderr)
        sys.exit(exc.returncode)
    except TimeoutError as exc:
        print(f"timeout: {exc}", file=sys.stderr)
        sys.exit(1)
    finally:
        if uart:
            uart.close()


if __name__ == "__main__":
    main()
