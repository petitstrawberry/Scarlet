"""Exercise exec-time library selection on an isolated Simulator guest disk."""
import argparse
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time
import uuid


def sim(*args):
    env = os.environ.copy()
    env.pop("DEVELOPER_DIR", None)
    env.pop("SDKROOT", None)
    return subprocess.check_output(
        ["/usr/bin/xcrun", "simctl", *args], text=True, env=env
    ).strip()


def boot(device, bundle, log, expected):
    sim("launch", f"--stderr={log}", device, bundle, "--boot-check")
    try:
        for _ in range(150):
            data = log.read_text(errors="replace") if log.exists() else ""
            if all(marker in data for marker in expected):
                print(" / ".join(expected), flush=True)
                return
            if "SCARLET_BOOT_FAIL" in data:
                break
            time.sleep(0.1)
        raise RuntimeError(f"Missing {expected} in console output:\n{data}")
    finally:
        sim("terminate", device, bundle)


def exercise(app, device):
    ident = "org.scarletos.linktest." + uuid.uuid4().hex
    # simctl redirects absolute paths under the host's temporary directory into
    # the simulated device. Keep console captures next to the build instead.
    with tempfile.TemporaryDirectory(dir=app.resolve().parent) as directory:
        tmp = Path(directory)
        copy = tmp / "Scarlet.app"
        shutil.copytree(app, copy)
        info = copy / "Info.plist"
        plist = plistlib.loads(info.read_bytes())
        plist["CFBundleIdentifier"] = ident
        info.write_bytes(plistlib.dumps(plist))
        subprocess.run(["codesign", "--force", "--sign", "-", copy], check=True)
        sim("install", device, str(copy))
        try:
            boot(device, ident, tmp / "a.log", ["SCARLET_LIBRARY_A", "SCARLET_BOOT_PASS"])
            data = Path(sim("get_app_container", device, ident, "data"))
            disk = data / "Library/Application Support/scarlet-root.ext2"

            def edit(commands):
                subprocess.run(
                    ["debugfs", "-w", "-f", "-", disk], input=commands + "\n",
                    text=True, check=True, stdout=subprocess.DEVNULL,
                )

            # Only the guest /lib lookup changes across these execs. The host
            # application, executable ELF, and frameworks stay installed once.
            edit("rm /lib\nmkdir /lib\nsymlink /lib/libboot-check.so /old_root/lib/libboot-check-alt.so")
            boot(device, ident, tmp / "b.log", ["SCARLET_LIBRARY_B", "SCARLET_BOOT_PASS"])
            edit("rm /lib/libboot-check.so")
            boot(device, ident, tmp / "missing.log", ["Cannot open shared library"])
            # A byte-distinct ELF with the same code is absent from the catalog.
            with (copy / "initramfs.cpio").open("rb") as archive:
                subprocess.run(["/usr/bin/cpio", "-id", "lib/libboot-check-alt.so"],
                               cwd=tmp, stdin=archive, check=True)
            altered = tmp / "lib/libboot-check-alt.so"
            with altered.open("ab") as out:
                out.write(b"!")
            edit(f'write "{altered}" /lib/libboot-check.so\nset_inode_field /lib/libboot-check.so mode 0100755')
            boot(device, ident, tmp / "unsigned.log", ["Shared library is absent from the signed iOS catalog"])
        finally:
            sim("uninstall", device, ident)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("--device", default="booted")
    args = parser.parse_args()
    exercise(args.app, args.device)
