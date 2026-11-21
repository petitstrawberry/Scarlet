"""Exercise native and Debian programs across two boots of the release image."""

import os
from pathlib import Path
import pty
import select
import shutil
import signal
import subprocess
import tempfile
import time


PROJECT = Path(__file__).resolve().parents[1]
IMAGES = PROJECT / ".scarlet/images"
BSP = PROJECT / "bsp/target/aarch64-apple-darwin/release/scarlet"


def wait_for(fd, token, pending):
    deadline = time.monotonic() + 90
    while token not in pending:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
            raise TimeoutError(f"Waiting for {token!r}. Recent console output: {pending[-800:]!r}")
        chunk = os.read(fd, 65536)
        if not chunk:
            raise EOFError(f"Console closed while waiting for {token!r}")
        pending = pending[-65536:] + chunk
        for line in pending.splitlines(keepends=True):
            if line.endswith(b"\n") and (
                b"[hosted] Task 1 stopped at " in line
                or b"(PID 1) caused unhandled user exception" in line
            ):
                raise RuntimeError(line.decode(errors="replace").strip())
    return pending.split(token, 1)[1]


def boot(executable, initramfs, root, first_boot):
    pid, fd = pty.fork()
    if pid == 0:
        env = dict(os.environ, SCARLET_HOSTED_GUI="0")
        os.execve(executable, [executable, initramfs, root], env)

    pending = b""
    try:
        for prompt, response in [(b"login:", b"root"), (b"Password:", b"")]:
            pending = wait_for(fd, prompt, pending)
            os.write(fd, response + b"\r")
        pending = wait_for(fd, b"root$ ", pending)
        if first_boot:
            os.write(fd, b"/bin/hello > /root/full-console-test\r")
            pending = wait_for(fd, b"\nroot$ ", pending)
        os.write(fd, b"/bin/cat /root/full-console-test\r")
        pending = wait_for(fd, b"Hello, world!", pending)
        pending = wait_for(fd, b"\nroot$ ", pending)
        os.write(fd, b'/systems/linux-aarch64/bin/dash -c "printf scarlet-pipe-47 | /usr/bin/tr a-z A-Z"\r')
        wait_for(fd, b"SCARLET-PIPE-47", pending)
        os.write(fd, b"\x1dq")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                pid = 0
                if os.waitstatus_to_exitcode(status):
                    raise RuntimeError(f"Scarlet exited with wait status {status}")
                return
            time.sleep(0.05)
        raise TimeoutError("Scarlet did not close after Ctrl-] q")
    finally:
        os.close(fd)
        if pid:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)


def main():
    # Keep the test root on the same volume so macOS can clone it cheaply.
    with tempfile.TemporaryDirectory(dir=IMAGES) as directory:
        # Workers exec the parent binary again for each address space. Keep both
        # boots on the same binary and initramfs during a concurrent build.
        executable = Path(directory) / "scarlet"
        initramfs = Path(directory) / "initramfs.cpio"
        os.link(BSP, executable)
        shutil.copyfile(IMAGES / "initramfs-aarch64-full.cpio", initramfs)
        root = Path(directory) / "root.ext2"
        source = IMAGES / "rootfs-aarch64-full.ext2"
        # The Nix shell also supplies GNU cp. Use the Darwin clone option.
        if subprocess.run(["/bin/cp", "-c", source, root]).returncode:
            shutil.copyfile(source, root)
        boot(executable, initramfs, root, first_boot=True)
        boot(executable, initramfs, root, first_boot=False)
    print("Passed native file persistence, Debian pipe, and console shutdown on both boots.")


if __name__ == "__main__":
    main()
