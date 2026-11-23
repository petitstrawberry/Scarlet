"""Select Scarlet ELF objects from the SDK's staged filesystems."""

import hashlib
import os
from pathlib import Path
import struct
import sys


def catalog(roots):
    seen = set()
    for root in roots:
        for source in sorted(root.rglob("*")):
            if source.is_symlink() or not source.is_file():
                continue
            with source.open("rb") as fd:
                header = fd.read(64)
                if not header.startswith(b"\x7fELF\x02\x01\x01S"):
                    continue
                if len(header) != 64:
                    raise ValueError(f"Truncated Scarlet ELF: {source}")
                kind, machine = struct.unpack_from("<HH", header, 16)
                if machine != 183 or kind == 1:  # AArch64, ET_REL
                    continue
                if kind == 2 and source.relative_to(root).as_posix() == "bin/scarlet-ld":
                    # The host loader supplies the interpreter implementation.
                    continue
                if kind != 3:  # ET_DYN
                    raise ValueError(f"Expected a Scarlet PIE or DSO: {source}")
                entry, phoff = struct.unpack_from("<QQ", header, 24)
                stride, count = struct.unpack_from("<HH", header, 54)
                if stride != 56:
                    raise ValueError(f"Invalid program header size: {source}")
                fd.seek(phoff)
                phdrs = fd.read(stride * count)
                if len(phdrs) != stride * count:
                    raise ValueError(f"Truncated program header table: {source}")
                interp = any(
                    struct.unpack_from("<I", phdrs, pos)[0] == 3  # PT_INTERP
                    for pos in range(0, len(phdrs), stride)
                )
                fd.seek(0)
                hasher = hashlib.sha256()
                for block in iter(lambda: fd.read(65536), b""):
                    hasher.update(block)
                digest = hasher.hexdigest()
            if digest in seen:
                continue
            seen.add(digest)
            mode = "--pie" if interp else "" if entry else "--dso"
            for field in (mode, digest, source):
                sys.stdout.buffer.write(os.fsencode(field) + b"\0")


if __name__ == "__main__":
    catalog([Path(root) for root in sys.argv[1:]])
