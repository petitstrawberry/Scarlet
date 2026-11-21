#!/usr/bin/env python3
"""Boot the hosted project and run its existing native hello ELF."""

import errno
import os
from pathlib import Path
import pty
import select
import signal
import time


PROJECT = Path(__file__).resolve().parents[1]


def wait_for(fd, token, pending, deadline):
    while token not in pending:
        remaining = max(0, deadline - time.monotonic())
        if not select.select([fd], [], [], remaining)[0]:
            raise TimeoutError(f"Waiting for {token!r}. Recent output: {pending[-4000:]!r}")
        try:
            chunk = os.read(fd, 65536)
        except OSError as error:
            if error.errno != errno.EIO:
                raise
            chunk = b""
        if not chunk:
            raise EOFError(f"Console closed before {token!r}. Recent output: {pending[-4000:]!r}")
        pending = pending[-65536:] + chunk
    return pending.split(token, 1)[1]


def smoke():
    pid, fd = pty.fork()
    if pid == 0:
        try:
            os.execvp("cargo", ["cargo", "scarlet", "run", "--project", str(PROJECT),
                                "--release", "--locked", "--no-image"])
        except OSError as error:
            os.write(2, f"Cannot launch cargo scarlet: {error}\n".encode())
            os._exit(127)

    deadline = time.monotonic() + 90
    pending = b""
    try:
        for prompt, reply in [(b"login: ", b"root\r"), (b"Password: ", b"\r"),
                              (b"root$ ", b"/bin/hello\r")]:
            pending = wait_for(fd, prompt, pending, deadline)
            os.write(fd, reply)
        for token in [b"Hello, world!", b"file = scarlet rust std", b"root$ "]:
            pending = wait_for(fd, token, pending, deadline)

        os.write(fd, b"\x1dq")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                pid = 0
                if os.waitstatus_to_exitcode(status) != 0:
                    raise RuntimeError(f"Scarlet runner exited with wait status {status}")
                return
            time.sleep(0.05)
        raise TimeoutError("Scarlet runner did not exit after Ctrl-] q")
    finally:
        os.close(fd)
        if pid:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)


if __name__ == "__main__":
    smoke()
    print("Passed boot, login, direct /bin/hello execution and terminal shutdown.")
