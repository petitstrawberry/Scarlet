#!/usr/bin/env python3
"""Prepare the pinned Android API 35 ARM64 SDK image for crosvm bring-up.

Requires Python lz4, mkfs.ext4, the Scarlet toolchain and build.py staging.
This is an experimental boot fixture, not an Android boot-completion test.
"""
import argparse
import gzip
import hashlib
import importlib.util
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import zipfile

import lz4.block

SOURCE = Path(__file__).resolve().parent
PROJECT = SOURCE.parent
ROOT = PROJECT.parents[1]
SHA1 = "2026a06409db630b56711afdbffb457c1dbaed49"
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("crosvm_fixture", Path(__file__).with_name("fixture.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


def legacy_lz4(data):
    result = bytearray()
    offset = 0
    while offset < len(data):
        size, = struct.unpack_from("<I", data, offset)
        offset += 4
        if size in (0, 0x184C2102):
            continue
        if size > len(data) - offset:
            raise ValueError("truncated legacy LZ4 block")
        result.extend(lz4.block.decompress(data[offset:offset + size], uncompressed_size=8 * 1024 * 1024))
        offset += size
    return result


def ramdisk_entries(data):
    entries = {}
    offset = 0
    while offset < len(data):
        while offset < len(data) and data[offset] == 0:
            offset += 1
        if offset == len(data):
            break
        if data[offset:offset + 6] != b"070701":
            raise ValueError(f"expected newc archive at {offset}")
        fields = [int(data[offset + 6 + i * 8:offset + 14 + i * 8], 16) for i in range(13)]
        size, namesize = fields[6], fields[11]
        name = bytes(data[offset + 110:offset + 110 + namesize - 1]).decode()
        start = (offset + 110 + namesize + 3) & ~3
        if start + size > len(data):
            raise ValueError("truncated newc entry")
        if name != "TRAILER!!!":
            entries[name] = (fields[1], bytes(data[start:start + size]))
        offset = (start + size + 3) & ~3
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-zip", type=Path, required=True, help="arm64-v8a-35_r02.zip")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--base-staging", type=Path, default=PROJECT / ".scarlet/base/staging")
    args = parser.parse_args()
    with args.sdk_zip.open("rb") as source:
        if hashlib.file_digest(source, "sha1").hexdigest() != SHA1:
            parser.error("SDK archive does not match API 35 ARM64 revision 02")
    output = args.output.resolve()
    # Refuse to recreate writable disks in an existing Android profile.
    assets = output / "android-assets"
    assets.mkdir(parents=True, exist_ok=False)
    with zipfile.ZipFile(args.sdk_zip) as sdk:
        for name in ["kernel-ranchu", "ramdisk.img", "system.img", "vendor.img", "NOTICE.txt", "source.properties"]:
            with sdk.open("arm64-v8a/" + name) as source, (assets / name).open("wb") as destination:
                shutil.copyfileobj(source, destination)
    with gzip.open(assets / "kernel-ranchu", "rb") as source, (assets / "Image").open("wb") as destination:
        shutil.copyfileobj(source, destination)
    entries = ramdisk_entries(legacy_lz4((assets / "ramdisk.img").read_bytes()))
    name = "first_stage_ramdisk/fstab.ranchu"
    mode, content = entries[name]
    fstab = content.decode().replace("/dev/block/vdc", "/dev/block/vdd")
    fstab = fstab.replace("/dev/block/platform/a003c00.virtio_mmio/by-name/metadata", "/dev/block/vdc")
    entries[name] = (mode, fstab.encode())
    with (assets / "initramfs.cpio").open("wb") as ramdisk:
        for inode, (name, (mode, content)) in enumerate(entries.items(), 1):
            smoke.archive_entry(ramdisk, name, mode, content, inode)
        smoke.archive_entry(ramdisk, "TRAILER!!!", 0, b"", len(entries) + 1)
        ramdisk.write(b"\0" * (-ramdisk.tell() % 512))
    for name, mib in [("metadata.img", 32), ("userdata.img", 512)]:
        path = assets / name
        with path.open("wb") as disk:
            disk.truncate(mib * 1024 * 1024)
        subprocess.run(["mkfs.ext4", "-F", str(path)], check=True)
    command = [sys.executable, str(SOURCE / "prepare-os.py"),
               "--base-staging", str(args.base_staging), "--output", str(output),
               "--image", str(assets / "Image"), "--initrd", str(assets / "initramfs.cpio"),
               "--mem", "1024", "--params",
               "earlycon=uart8250,mmio,0x3f8 console=hvc0 rdinit=/init androidboot.hardware=ranchu "
               "androidboot.console=hvc0 androidboot.selinux=permissive nokaslr loglevel=7 "
               "panic=-1 ignore_loglevel printk.devkmsg=on androidboot.boot_devices=10000.pci",
               "--serial", "type=stdout,hardware=serial,num=1,console=false,stdin=false",
               "--serial", "type=stdout,hardware=virtio-console,num=1,console=true,stdin=false"]
    for name in ["system.img", "vendor.img"]:
        command += ["--read-only-disk", str(assets / name)]
    for name in ["metadata.img", "userdata.img"]:
        command += ["--disk", str(assets / name)]
    subprocess.run(command, check=True)


if __name__ == "__main__":
    main()
