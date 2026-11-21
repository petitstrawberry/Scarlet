"""Check normal SAS playback and no-input exit on a private full-image boot."""

import argparse
import hashlib
import math
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import time
import wave


PROJECT = Path(__file__).resolve().parents[1]
IMAGES = PROJECT / ".scarlet/images"
BSP = PROJECT / "bsp/target/aarch64-apple-darwin/release/scarlet"


def tone(path, rate, channels, frames):
    with wave.open(str(path), "wb") as output:
        output.setparams((channels, 2, rate, frames, "NONE", "not compressed"))
        output.writeframes(b"".join(
            struct.pack("<h", int(2000 * math.sin(2 * math.pi * 440 * n / rate))) * channels
            for n in range(frames)
        ))


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def boot(executable, initramfs, root, log):
    pid, fd = pty.fork()
    if pid == 0:
        # Keep the ordinary runner defaults: GUI enabled and four vCPUs.
        env = dict(os.environ)
        env.pop("SCARLET_HOSTED_GUI", None)
        env.pop("SCARLET_HOSTED_CPUS", None)
        os.execve(executable, [str(executable), str(initramfs), str(root)], env)

    pending = b""

    def wait(token, timeout=90):
        nonlocal pending
        deadline = time.monotonic() + timeout
        while token not in pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
                raise TimeoutError(f"Waiting for {token!r}; tail={pending[-1200:]!r}")
            chunk = os.read(fd, 65536)
            if not chunk:
                raise EOFError(f"Console closed while waiting for {token!r}")
            log.write(chunk)
            pending = (pending + chunk)[-1024 * 1024:]
        before, pending = pending.split(token, 1)
        return before + token

    def command(text, timeout=90):
        nonlocal pending
        pending = b""
        os.write(fd, text.encode() + b"\r")
        return wait(b"\nroot$ ", timeout)

    try:
        for prompt, reply in [(b"login:", b"root"), (b"Password:", b"")]:
            wait(prompt)
            os.write(fd, reply + b"\r")
        wait(b"root$ ")
        command("/bin/sasctl status")
        for name in ["stereo", "mono-tail", "stereo"]:
            started = time.monotonic()
            output = command(f"/bin/mplayer --sas /root/audio-{name}.wav", 45)
            if b"mplayer: finished" not in output:
                raise AssertionError(f"Playback did not finish: {output[-1200:]!r}")
            if name == "stereo" and b"00:01 / 00:02" not in output:
                raise AssertionError("Stereo playback did not report advancing beyond 0%")
            print(f"PASS {name}: progressed, finished and returned without input in "
                  f"{time.monotonic() - started:.3f}s", flush=True)
        print(command("/bin/logctl -u sas -n 40").decode(errors="replace"), flush=True)
        os.write(fd, b"\x1dq")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                pid = 0
                if os.waitstatus_to_exitcode(status):
                    raise RuntimeError(f"BSP exited with status {status}")
                return
            time.sleep(0.05)
        raise TimeoutError("BSP did not close after Ctrl-] q")
    except Exception:
        # Preserve server evidence if a client stalls. Interrupt only this
        # boot's foreground job, then ask its ordinary journal service.
        try:
            os.write(fd, b"\x03")
            wait(b"\nroot$ ", 10)
            print(command("/bin/logctl -u sas -n 80", 15).decode(errors="replace"), flush=True)
        except Exception as diagnostic_error:
            print(f"Could not collect SAS journal: {diagnostic_error}", flush=True)
        raise
    finally:
        if pid:
            try:
                os.write(fd, b"\x1dq")
                time.sleep(0.2)
                os.killpg(pid, signal.SIGKILL)
            except (ProcessLookupError, OSError):
                pass
            os.waitpid(pid, 0)
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", type=Path, default=Path("/tmp/scarlet-full-audio.log"))
    options = parser.parse_args()
    debugfs = shutil.which("debugfs")
    if not debugfs:
        raise RuntimeError("Run inside the repository Nix development shell (debugfs required)")
    with tempfile.TemporaryDirectory(prefix="audio-acceptance-", dir=IMAGES) as directory:
        directory = Path(directory)
        executable = directory / "scarlet"
        initramfs = directory / "initramfs.cpio"
        root = directory / "root.ext2"
        os.link(BSP, executable)
        shutil.copyfile(IMAGES / "initramfs-aarch64-full.cpio", initramfs)
        if subprocess.run(["/bin/cp", "-c", IMAGES / "rootfs-aarch64-full.ext2", root]).returncode:
            shutil.copyfile(IMAGES / "rootfs-aarch64-full.ext2", root)
        for name, rate, channels, frames in [
            ("stereo", 48000, 2, 96000),
            ("mono-tail", 44100, 1, 4410 + 17),
        ]:
            fixture = directory / f"audio-{name}.wav"
            tone(fixture, rate, channels, frames)
            subprocess.run([debugfs, "-w", "-R", f"write {fixture} /root/{fixture.name}", root],
                           check=True, capture_output=True)
            listing = subprocess.check_output(
                [debugfs, "-R", f"stat /root/{fixture.name}", root], stderr=subprocess.STDOUT
            )
            if b"Inode:" not in listing:
                raise RuntimeError(f"Fixture installation failed: {listing!r}")
        for artifact in [executable, initramfs, root]:
            print(f"SHA256 {artifact.name} {digest(artifact)}", flush=True)
        with options.log.open("wb", buffering=0) as log:
            boot(executable, initramfs, root, log)
    print(f"Passed full-image SAS playback and no-input shutdown; log: {options.log}")


if __name__ == "__main__":
    main()
