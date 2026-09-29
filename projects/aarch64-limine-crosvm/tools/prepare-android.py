#!/usr/bin/env python3
"""Prepare the pinned Android API 35 ARM64 SDK image for crosvm bring-up.

Requires Python lz4, mkfs.ext4, debugfs, the Scarlet toolchain and build.py staging.
This is an experimental boot fixture; Android UI support is incomplete.
"""
import argparse
import gzip
import hashlib
import importlib.util
from pathlib import Path
import mmap
import re
import shutil
import struct
import subprocess
import sys
import tempfile
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


def crosvm_fstab(content):
    """Map mount points consistently in both boot stages, preserving file size."""
    devices = {b"/data": b"/dev/block/vdd", b"/metadata": b"/dev/block/vdc"}
    seen = set()

    def replace(match):
        prefix, source, spacing, mount = match.groups()
        if mount not in devices or source.startswith(b"#"):
            return match.group()
        seen.add(mount)
        # Keep vendor inode size, extents and SELinux attributes unchanged.
        field = devices[mount].ljust(len(source) + len(spacing))
        if len(field) != len(source) + len(spacing) or not field.endswith(b" "):
            raise ValueError("fstab source field is too short")
        return prefix + field + mount

    result = re.sub(rb"^([ \t]*)([^\s#]+)([ \t]+)(\S+)", replace, content, flags=re.M)
    if seen != set(devices):
        raise ValueError("fstab must contain /data and /metadata")
    return result


def patch_disk_fstab(image, original):
    """Patch the exact fstab payload in the checksum-pinned SDK disk copy.

    The SDK includes vendor both in vendor.img and as a logical partition in
    system.img's super partition. Equal-length data edits leave GPT/LP metadata,
    ext4 inode attributes and SELinux labels intact. Matching the entire file
    avoids relying on fixed partition offsets or host Android image tools.
    """
    patched = crosvm_fstab(original)
    with image.open("r+b") as disk, mmap.mmap(disk.fileno(), 0) as data:
        start = data.find(original)
        if start < 0 or data.find(original, start + 1) >= 0:
            raise ValueError(f"expected one complete vendor fstab in {image}")
        data[start:start + len(original)] = patched
        data.flush()


def vendor_files(image, names):
    """Read files from the pinned SDK's vendor GPT partition using debugfs."""
    with tempfile.TemporaryDirectory(prefix="scarlet-android-vendor-") as directory:
        directory = Path(directory)
        partition = directory / "vendor.ext4"
        with image.open("rb") as disk:
            disk.seek(512)
            header = disk.read(92)
            if header[:8] != b"EFI PART":
                raise ValueError("expected vendor GPT image")
            table, count, size = struct.unpack_from("<QII", header, 72)
            if count > 1024 or size < 128 or size > 4096:
                raise ValueError("invalid vendor GPT entry table")
            disk.seek(table * 512)
            entries = [disk.read(size) for _ in range(count)]
            vendors = [entry for entry in entries
                       if entry[56:128].decode("utf-16-le").rstrip("\0") == "vendor"]
            if len(vendors) != 1:
                raise ValueError("expected one vendor GPT partition")
            first, last = struct.unpack_from("<QQ", vendors[0], 32)
            remaining = (last - first + 1) * 512
            if first > last or (last + 1) * 512 > image.stat().st_size:
                raise ValueError("invalid vendor GPT extent")
            disk.seek(first * 512)
            with partition.open("wb") as output:
                while remaining:
                    block = disk.read(min(remaining, 1024 * 1024))
                    if not block:
                        raise ValueError("truncated vendor GPT partition")
                    output.write(block)
                    remaining -= len(block)
        result = {}
        for index, name in enumerate(names):
            target = directory / str(index)
            subprocess.run(["debugfs", "-R", f"dump {name} {target}", str(partition)],
                           check=True, capture_output=True)
            # debugfs can return zero even when its command failed.
            result[name] = target.read_bytes()
        return result


def patch_vendor_payloads(image, patches):
    """Replace complete, unique file payloads without changing inode metadata."""
    with image.open("r+b") as disk, mmap.mmap(disk.fileno(), 0) as data:
        edits = []
        for original, replacement in patches:
            if not original or len(replacement) > len(original):
                raise ValueError("vendor replacement must fit the original file")
            start = data.find(original)
            if start < 0 or data.find(original, start + 1) >= 0:
                raise ValueError(f"expected one complete vendor payload in {image}")
            edits.append((start, replacement.ljust(len(original), b" ")))
        for start, replacement in edits:
            data[start:start + len(replacement)] = replacement
        data.flush()


def diagnostic_init(original):
    # Reclaim comment space to preserve the pinned vendor inode and its label.
    extra = (PROJECT / "fixtures/android-diagnostics.rc").read_bytes()
    compact = b"\n".join(line for line in (original + extra).splitlines()
                         if not line.lstrip().startswith(b"#")) + b"\n"
    # Kernel errors already reach hvc0. Replaying the kernel buffer on hvc1
    # duplicates them and delays the userspace crash reports we need here.
    old = b"/system/bin/logcat -f /dev/hvc1 ${ro.boot.logcat}"
    if compact.count(old) != 1:
        raise ValueError("expected one Ranchu logcat service")
    return compact.replace(old, b"/system/bin/logcat -b main -b system -b crash -f /dev/hvc1 ${ro.boot.logcat}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-zip", type=Path, required=True, help="arm64-v8a-35_r02.zip")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--base-staging", type=Path, default=PROJECT / ".scarlet/base/staging")
    parser.add_argument("--ranchu-sensors", action="store_true",
                        help="retain the Ranchu sensors sub-HAL (requires its host transport)")
    parser.add_argument("--gpu-2d", action="store_true",
                        help="attach experimental virtio-gpu 2D; requires a GPU-enabled crosvm")
    args = parser.parse_args()
    for tool in ("debugfs", "mkfs.ext4"):
        if shutil.which(tool) is None:
            parser.error(f"required host tool not found: {tool}")
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
    entries[name] = (mode, crosvm_fstab(content))
    for disk_name in ["system.img", "vendor.img"]:
        patch_disk_fstab(assets / disk_name, content)
    files = vendor_files(assets / "vendor.img",
                         ["/etc/sensors/hals.conf", "/etc/init/hw/init.ranchu.rc"])
    rc = files["/etc/init/hw/init.ranchu.rc"]
    patches = [(rc, diagnostic_init(rc))]
    if not args.ranchu_sensors:
        # HalProxy accepts an empty sub-HAL list and reports zero sensors.
        # This file is whitespace-tokenized; it does NOT support comments.
        patches.append((files["/etc/sensors/hals.conf"], b"\n"))
    for disk_name in ["system.img", "vendor.img"]:
        patch_vendor_payloads(assets / disk_name, patches)
    with (assets / "initramfs.cpio").open("wb") as ramdisk:
        for inode, (name, (mode, content)) in enumerate(entries.items(), 1):
            smoke.archive_entry(ramdisk, name, mode, content, inode)
        smoke.archive_entry(ramdisk, "TRAILER!!!", 0, b"", len(entries) + 1)
        ramdisk.write(b"\0" * (-ramdisk.tell() % 512))
    for name, mib in [("metadata.img", 32), ("userdata.img", 512)]:
        path = assets / name
        with path.open("wb") as disk:
            disk.truncate(mib * 1024 * 1024)
        # Android's bundled e2fsprogs 1.46.6 cannot handle orphan_file from
        # newer host defaults (in particular orphan_present while mounted).
        subprocess.run(["mkfs.ext4", "-F", "-O", "^orphan_file", str(path)], check=True)
    command = [sys.executable, str(SOURCE / "prepare-os.py"),
               "--base-staging", str(args.base_staging), "--output", str(output),
               "--image", str(assets / "Image"), "--initrd", str(assets / "initramfs.cpio"),
               "--mem", "2048", "--params",
               "earlycon=uart8250,mmio,0x3f8 console=hvc0 rdinit=/init androidboot.hardware=ranchu "
               "androidboot.console=hvc0 androidboot.logcat=*:E androidboot.selinux=permissive nokaslr loglevel=4 "
               "panic=-1 printk.devkmsg=on androidboot.boot_devices=10000.pci",
               "--serial", "type=stdout,hardware=serial,num=1,console=false,stdin=false",
               "--serial", "type=stdout,hardware=virtio-console,num=1,console=true,stdin=false",
               # ranchu's goldfish-logcat writes to /dev/hvc1, separate from hvc0.
               "--serial", "type=stdout,hardware=virtio-console,num=2,console=false,stdin=false"]
    for name in ["system.img", "vendor.img"]:
        command += ["--read-only-disk", str(assets / name)]
    for name in ["metadata.img", "userdata.img"]:
        command += ["--disk", str(assets / name)]
    if args.gpu_2d:
        command += ["--gpu", "backend=2d,width=800,height=600"]
    subprocess.run(command, check=True)


if __name__ == "__main__":
    main()
