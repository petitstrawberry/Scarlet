"""Boot native PIEs through the shipped scarlet-ld. Run in the project dev shell."""

import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time


PROJECT = Path(__file__).resolve().parents[1]
BSP = PROJECT / "bsp/target/aarch64-apple-darwin/release/scarlet"
INITRAMFS = PROJECT / ".scarlet/images/initramfs-aarch64-full.cpio"
LOADER = PROJECT / ".scarlet/staging/initramfs/bin/scarlet-ld"
SOURCE = PROJECT / "bsp/tests/fixtures/elf_page_quarters.c"
PHDR = struct.Struct("<IIQQQQQQ")


def add_relro(image, data):
    # Repurpose the redundant PT_GNU_STACK header, leaving every PT_LOAD and
    # its 16 KiB alignment intact. The following 4 KiB must remain writable.
    symbols = subprocess.check_output(["llvm-nm", "-P", image], text=True)
    address = next(int(s.split()[2], 16) for s in symbols.splitlines()
                   if s.split()[0] == "relro_pair")
    phoff, = struct.unpack_from("<Q", data, 32)
    size, count = struct.unpack_from("<HH", data, 54)
    assert size == PHDR.size and address % 16384 == 0
    headers = [(i, PHDR.unpack_from(data, i)) for i in range(phoff, phoff+size*count, size)]
    slot = next(i for i, h in headers if h[0] == 0x6474e551)
    load = next(h for _, h in headers if h[0] == 1 and h[1] == 6 and
                h[3] <= address and address + 8192 <= h[3] + h[5])
    PHDR.pack_into(data, slot, 0x6474e552, 4, load[2] + address - load[3],
                   address, address, 4096, 4096, 1)


def make_root(work, alignment, pagesz, relro):
    root = work / "root"
    for name in ("bin", "dev", "etc", "proc", "sys", "tmp", "home", "shared", "root"):
        (root/name).mkdir(parents=True)
    image = root / "bin/stemd"  # Normal init execs this path after mounting root.
    obj = work / "guest.o"
    subprocess.run(["clang", "--target=aarch64-none-elf", "-O2", "-ffreestanding",
                    "-fno-stack-protector", "-fPIC", f"-DEXPECT_PAGESZ={pagesz}",
                    *(["-DTEST_RELRO"] if relro else []), "-c", SOURCE, "-o", obj], check=True)
    subprocess.run(["ld.lld", "-pie", "-e", "_start", "-z", "norelro", "-z",
                    f"max-page-size={alignment}", "--dynamic-linker", "/bin/scarlet-ld",
                    obj, "-o", image], check=True)
    data = bytearray(image.read_bytes())
    data[7] = 83  # ELFOSABI_SCARLET
    if relro:
        add_relro(image, data)
    image.write_bytes(data)
    shutil.copy2(LOADER, root/"bin/scarlet-ld")
    disk = work/"guest.ext2"
    with disk.open("wb") as f:
        f.truncate(12 << 20)
    subprocess.run(["mke2fs", "-q", "-F", "-t", "ext2", "-d", root, disk], check=True)
    return disk


def boot(disk, pagesz, relro):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["SCARLET_HOSTED_GUI"] = "0"
        os.execv(BSP, [BSP, INITRAMFS, disk])
    output = bytearray()
    try:
        deadline = time.monotonic() + 60
        while b"QUARTER PASS\r\n" not in output:
            if b"QUARTER FAIL" in output or time.monotonic() >= deadline:
                raise AssertionError(output[-2000:].decode(errors="replace"))
            if select.select([fd], [], [], 0.5)[0]:
                chunk = os.read(fd, 65536)
                if not chunk:
                    raise EOFError(output[-2000:].decode(errors="replace"))
                output.extend(chunk)
        assert f"AT_PAGESZ={pagesz}\r\n".encode() in output
        assert output.count(b"User mapping protection violation") >= 1 + int(relro)
        assert b"Unmapped user address" in output
        sys.stdout.buffer.write(output)
    finally:
        os.close(fd)
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)


if __name__ == "__main__":
    for name, alignment, pagesz, relro in [
        ("eligible PT_LOAD", 16384, 16384, False),
        ("4 KiB PT_LOAD", 4096, 4096, False),
        ("partial GNU_RELRO", 16384, 4096, True),
    ]:
        print(name, flush=True)
        with tempfile.TemporaryDirectory(prefix="scarlet-elf-quarters-") as tmp:
            boot(make_root(Path(tmp), alignment, pagesz, relro), pagesz, relro)
