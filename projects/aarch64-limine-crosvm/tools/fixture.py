#!/usr/bin/env python3
"""Private crosvm project archive and QEMU verification helpers."""

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


SUCCESS = re.compile(rb"\nSCARLET_CROSVM_GUEST_OK\r?\n")
FAILURE = re.compile(rb"\nSCARLET_CROSVM_FAIL(?:[ :\r\n]|$)")
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


def activate_profile(profile):
    """Select a completed fixture for the project's normal image/run commands."""
    import shutil
    import uuid

    profile = Path(profile).resolve()
    boot = profile / "boot"
    for name in ["bin", "dev", "mnt/newroot", "home", "shared", "tmp", "root"]:
        (boot / name).mkdir(parents=True, exist_ok=True)
    for name in ["init", "bin/scarlet-ld"]:
        shutil.copy2(profile / "staging" / name, boot / name)
    state = Path(__file__).resolve().parents[1] / ".scarlet"
    state.mkdir(exist_ok=True)
    active = state / "active"
    if active.exists() and not active.is_symlink():
        raise ValueError(f"refusing to replace non-symlink profile: {active}")
    temporary = state / (".active-" + uuid.uuid4().hex)
    try:
        temporary.symlink_to(profile, target_is_directory=True)
        temporary.replace(active)
    finally:
        temporary.unlink(missing_ok=True)


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
                failure_deadline = None
                while True:
                    now = time.monotonic()
                    if failure_deadline is not None and now >= failure_deadline:
                        break
                    remaining = timeout - (now - started)
                    if remaining <= 0:
                        if failure_deadline is None:
                            result = "timeout waiting for the success marker"
                        break
                    if failure_deadline is not None:
                        remaining = min(remaining, failure_deadline - now)
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
                    if failure_deadline is not None:
                        continue
                    if PANIC.search(tail):
                        result = "guest panic"
                    elif FAILURE.search(tail):
                        result = "fixture failure marker"
                    elif INTERPRETER_ERROR.search(tail):
                        result = "interpreter error"
                    elif SUCCESS.search(tail):
                        result = "PASS"
                        break
                    else:
                        tail = tail[-512:]
                        continue
                    # A serial marker may arrive before its diagnostic text.
                    # Drain briefly, keeping the first failure authoritative.
                    failure_deadline = time.monotonic() + 0.25
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
