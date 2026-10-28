#!/usr/bin/env python3
"""Extract ARM64 Bionic runtime files from an uncompressed runtime APEX.

Only the explicitly named runtime files are exported. Uses read-only debugfs;
does not mount images or copy proprietary SDK applications.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apex", type=Path, required=True, help="com.android.runtime.apex with ext4 apex_payload.img")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not shutil.which("debugfs"):
        parser.error("debugfs is required; use nix develop")
    if args.output.exists():
        parser.error("output already exists")
    output = args.output.resolve()
    with zipfile.ZipFile(args.apex) as archive:
        info = archive.getinfo("apex_payload.img")
        if info.file_size > 128 * 1024 * 1024:
            parser.error("runtime payload exceeds 128 MiB")
        output.mkdir(parents=True)
        with tempfile.TemporaryDirectory(prefix="runtime-", dir=output) as temporary:
            image = Path(temporary) / "runtime.ext4"
            with archive.open(info) as source, image.open("xb") as dest:
                shutil.copyfileobj(source, dest)
            for name in ["bin/linker64", "lib64/bionic/libc.so", "lib64/bionic/libdl.so",
                         "lib64/bionic/libm.so", "lib64/bionic/libdl_android.so"]:
                target = output / "apex/com.android.runtime" / name
                target.parent.mkdir(parents=True, exist_ok=True)
                quoted = str(target).replace("\\", "\\\\").replace('"', '\\"')
                subprocess.run(["debugfs", "-R", f'dump /{name} "{quoted}"', str(image)], check=True,
                               stdout=subprocess.DEVNULL)
                # debugfs returns success even when its internal command fails.
                header = target.read_bytes()[:64] if target.is_file() else b""
                if len(header) < 64 or header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 183:
                    raise ValueError(f"missing or non-ARM64 runtime ELF: {name}")
                target.chmod(0o755)
    link = output / "system/bin/linker64"
    link.parent.mkdir(parents=True)
    link.symlink_to("../../apex/com.android.runtime/bin/linker64")
    metadata = {"apex_sha256": hashlib.sha256(args.apex.read_bytes()).hexdigest(),
                "files": {str(p.relative_to(output)): hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in output.rglob("*") if p.is_file() and not p.is_symlink()}}
    (output / "runtime.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
