"""Run guest instruction fixtures on the release BSP in the project dev shell."""

import os
from pathlib import Path
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import time


BSP = Path(__file__).resolve().parents[1] / "bsp"


def assemble(work, source, sections, strip_sections=False):
    script = "ENTRY(_start)\nPHDRS { text PT_LOAD FLAGS(5); data PT_LOAD FLAGS(6); }\n"
    (work / "guest.ld").write_text(script + "SECTIONS { " + sections + " }\n")
    subprocess.run(
        ["clang", "-target", "aarch64-none-elf", "-DSCARLET_NATIVE", "-c",
         BSP / "tests/fixtures" / source, "-o", "guest.o"],
        cwd=work, check=True,
    )
    subprocess.run(
        ["ld.lld", "-z", "max-page-size=16384", "-T", "guest.ld", "guest.o", "-o", "init"],
        cwd=work, check=True,
    )
    elf = work / "init"
    data = bytearray(elf.read_bytes())
    data[7] = 83  # ELFOSABI_SCARLET
    if strip_sections:
        struct.pack_into("<Q", data, 40, 0)  # e_shoff
        struct.pack_into("<HHH", data, 58, 0, 0, 0)  # e_shentsize, e_shnum, e_shstrndx
    elf.write_bytes(data)
    with (work / "init.cpio").open("wb") as output:
        subprocess.run(
            ["cpio", "--quiet", "-o", "-H", "newc"], input=b"init\n",
            cwd=work, stdout=output, check=True,
        )


def check_exit_trap(work):
    # PID 1 tests leave the kernel online. Its native Exit syscall reports
    # success only after the guest has checked its registers and memory.
    child = subprocess.Popen(
        [work / "scarlet", work / "init.cpio", work / "root.ext2"],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        env=dict(os.environ, SCARLET_HOSTED_GUI="0", SCARLET_TRACE_USER_TRAPS="1"),
        start_new_session=True,
    )
    pending = b""
    try:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if not select.select([child.stdout], [], [], 1)[0]:
                continue
            data = os.read(child.stdout.fileno(), 32768)
            if not data:
                break
            pending = pending[-32768:] + data
            if b" syscall=1 args=[0," in pending:
                return
            if any(marker in pending for marker in (
                b" syscall=1 args=[1,", b"Task 1 stopped at", b"caused unhandled user exception",
            )):
                break
        raise RuntimeError(pending.decode(errors="replace")[-2500:])
    finally:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
        child.stdout.close()


def main():
    with tempfile.TemporaryDirectory(prefix="scarlet-guest-instructions-") as tmp:
        work = Path(tmp)
        shutil.copy2(BSP / "target/aarch64-apple-darwin/release/scarlet", work / "scarlet")
        with (work / "root.ext2").open("wb") as disk:
            disk.truncate(16 << 20)
        subprocess.run(["mke2fs", "-q", "-t", "ext2", "-F", "root.ext2"], cwd=work, check=True)
        start = ". = 0x7800800000; "
        assemble(work, "anonymous_jit.S", start + ".text : { *(.text*) *(.rodata*) } :text", True)
        check_exit_trap(work)
        print("Sectionless ELF and anonymous JIT passed.", flush=True)
        for name, text_whole, data_whole in (
            ("mediated text and data", False, False),
            ("native text and mediated data", True, False),
            ("native text and data", True, True),
        ):
            align = ". = ALIGN(16384);"
            text = ".text : { *(.text*) " + (align if text_whole else "") + " } :text "
            address = ". = 0x7800810000; " if data_whole else ". = 0x7800811000; "
            data = ".data : { *(.data*) " + (align if data_whole else "") + " } :data"
            assemble(work, "pair_cas.S", start + text + address + data)
            check_exit_trap(work)
            print(f"CASP with {name} passed.", flush=True)


if __name__ == "__main__":
    main()
