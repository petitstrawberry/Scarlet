#!/usr/bin/env python3
"""Stage pinned Android 17 Cuttlefish with guest ANGLE/SwiftShader and virtio-gpu 2D.

Experimental, permissive, AVB-disabled profile. Does not select the active project
profile. Requires Python lz4, mkfs.ext4 and the Scarlet toolchain; no Android SDK
build, privileged mounts or Docker needed. Existing profiles are never replaced.
"""
import argparse
import hashlib
import importlib.util
import json
import mmap
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import uuid
import zipfile
import zlib

sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parent
PROJECT = SOURCE.parent
BUILD = "16373615"
TARGET = "aosp_cf_arm64_only_phone-userdebug"
ARCHIVE_MD5 = "46f44b3494d54d64c39af2df619f9df3"
ARCHIVE_SHA256 = "051caf8072ba9fb417e05999de2984752e44e13ce70b6c49c669f0a73db85c18"
ARCHIVE_SIZE = 1101175103
MIB = 1024 * 1024
spec = importlib.util.spec_from_file_location("android_fixture", SOURCE / "prepare-android.py")
android = importlib.util.module_from_spec(spec)
spec.loader.exec_module(android)


def align(value, size):
    return (value + size - 1) // size * size


def copy_sparse(source, destination, length):
    """Copy exact bytes, retaining large zero regions as holes."""
    while length:
        data = source.read(min(MIB, length))
        if not data:
            raise ValueError("truncated image")
        if data.count(0) == len(data):
            destination.seek(len(data), 1)
        else:
            destination.write(data)
        length -= len(data)


def unsparse(source, destination):
    header = source.read(28)
    magic, major, minor, hsize, csize, block, blocks, chunks, checksum = struct.unpack("<I4H4I", header)
    if magic != 0xED26FF3A or major != 1 or hsize < 28 or csize < 12 or block != 4096:
        raise ValueError("unsupported Android sparse image")
    source.read(hsize - 28)
    with destination.open("xb") as out:
        for _ in range(chunks):
            kind, _, count, size = struct.unpack("<2H2I", source.read(12))
            source.read(csize - 12)
            length = count * block
            if kind == 0xCAC1 and size == csize + length:
                copy_sparse(source, out, length)
            elif kind == 0xCAC2 and size == csize + 4:
                word = source.read(4)
                if len(word) != 4:
                    raise ValueError("truncated fill chunk")
                if word == b"\0" * 4:
                    out.seek(length, 1)
                else:
                    buf = word * (MIB // 4)
                    while length:
                        part = buf[:min(length, MIB)]
                        out.write(part)
                        length -= len(part)
            elif kind == 0xCAC3 and size == csize:
                out.seek(length, 1)
            elif kind == 0xCAC4 and size == csize + 4 and count == 0:
                if len(source.read(4)) != 4:
                    raise ValueError("truncated CRC chunk")
            else:
                raise ValueError(f"unsupported sparse chunk {kind:#x}")
        if out.tell() != blocks * block or source.read(1):
            raise ValueError("sparse image size mismatch")
        out.truncate()


def logical_extent(image, name):
    """Read the pinned image's primary liblp table; require one linear extent."""
    with image.open("rb") as f:
        f.seek(12288)
        header = f.read(256)
        if header[:4] != b"0PLA" or struct.unpack_from("<I", header, 8)[0] != 256:
            raise ValueError("unexpected super metadata")
        table_size = struct.unpack_from("<I", header, 44)[0]
        if table_size > 65536 - 256:
            raise ValueError("oversized super table")
        tables = f.read(table_size)
        part_offset, part_count, part_size = struct.unpack_from("<III", header, 80)
        ext_offset, ext_count, ext_size = struct.unpack_from("<III", header, 92)
        if part_size != 52 or ext_size != 24:
            raise ValueError("unexpected liblp record sizes")
        for i in range(part_count):
            p = tables[part_offset + i * part_size:part_offset + (i + 1) * part_size]
            if p[:36].split(b"\0")[0].decode() != name:
                continue
            first, count = struct.unpack_from("<II", p, 40)
            if count != 1 or first >= ext_count:
                raise ValueError("expected one logical extent")
            sectors, kind, start, device = struct.unpack_from("<QIQI", tables, ext_offset + first * ext_size)
            if kind != 0 or device != 0 or (start + sectors) * 512 > image.stat().st_size:
                raise ValueError("invalid logical extent")
            return start * 512, sectors * 512
    raise ValueError(f"logical partition missing: {name}")


def patch_vendor(super_image):
    """Edit unique uncompressed EROFS payloads without rebuilding metadata/xattrs."""
    start, size = logical_extent(super_image, "vendor_a")
    with super_image.open("r+b") as f, mmap.mmap(f.fileno(), 0) as data:
        if data[start + 1024:start + 1028] != struct.pack("<I", 0xE0F5E1E2):
            raise ValueError("expected EROFS vendor")
        prefix = b"on early-init\n#    loglevel 8\n\n    setprop ro.sf.lcd_density"
        suffix = b"    setprop ro.zygote.disable_gl_preload ${ro.boot.hardware.guest_disable_renderer_preload}\n"
        a = data.find(prefix, start, start + size)
        b = data.find(suffix, a, start + size) if a >= 0 else -1
        if a < 0 or b < a or data.find(prefix, a + 1, start + size) >= 0:
            raise ValueError("pinned graphics init payload not found uniquely")
        original = bytes(data[a:b + len(suffix)])
        replacement = (PROJECT / "fixtures/cuttlefish-graphics.rc").read_bytes()
        serial = (b"service seriallogging /system/bin/logcat -b all -v threadtime -f /dev/hvc2 *:V\n"
                  b"    class main\n    user logd\n    group root logd\n    seclabel u:r:logpersist:s0\n")
        # Kernel errors already arrive on hvc0; avoid replaying that buffer.
        edited_serial = serial.replace(b"-b all -v threadtime", b"-b main,system,crash").replace(b"*:V", b"*:E")
        edits = []
        # No Cuttlefish sensors-control host channel is attached. HalProxy
        # accepts an empty (whitespace-only) list and reports no sensors.
        sensors = b"android.hardware.sensors@2.1-impl.cuttlefish.so\n"
        for old, new in [(original, replacement), (serial, edited_serial), (sensors, b"\n")]:
            pos = data.find(old, start, start + size)
            if len(new) > len(old) or pos < 0 or data.find(old, pos + 1, start + size) >= 0:
                raise ValueError("replacement must fit a unique complete vendor payload")
            edits.append((pos, new.ljust(len(old), b" ")))
        for pos, new in edits:
            data[pos:pos + len(new)] = new
        data.flush()


def gpt_image(path, partitions):
    """Create a sparse GPT disk from (partition name, raw image) inputs."""
    entries = bytearray(128 * 128)
    cursor = 2048
    copies = []
    for i, (name, source) in enumerate(partitions):
        length = source.stat().st_size
        if length % 512 or i >= 128 or len(name.encode("utf-16le")) > 72:
            raise ValueError("invalid GPT partition")
        end = cursor + length // 512 - 1
        entry = (uuid.UUID("0fc63daf-8483-4772-8e79-3d69d8477de4").bytes_le + uuid.uuid4().bytes_le +
                 struct.pack("<QQQ", cursor, end, 0) + name.encode("utf-16le").ljust(72, b"\0"))
        entries[i * 128:(i + 1) * 128] = entry
        copies.append((cursor * 512, source))
        cursor = align(end + 1, 2048)
    sectors = cursor + 2048
    disk_id = uuid.uuid4().bytes_le
    crc = zlib.crc32(entries)

    def header(current, backup, table):
        h = bytearray(struct.pack("<8sIIIIQQQQ16sQIII", b"EFI PART", 0x10000, 92, 0, 0,
                                 current, backup, 34, sectors - 34, disk_id, table, 128, 128, crc))
        struct.pack_into("<I", h, 16, zlib.crc32(h))
        return h.ljust(512, b"\0")

    with path.open("xb") as out:
        mbr = bytearray(512)
        mbr[446:462] = struct.pack("<B3sB3sII", 0, b"\0\2\0", 0xEE, b"\xff" * 3, 1, min(sectors - 1, 0xffffffff))
        mbr[510:] = b"\x55\xaa"
        out.write(mbr)
        out.write(header(1, sectors - 1, 2))
        out.write(entries)
        for offset, source in copies:
            out.seek(offset)
            with source.open("rb") as f:
                copy_sparse(f, out, source.stat().st_size)
        out.seek((sectors - 33) * 512)
        out.write(entries)
        out.write(header(sectors - 1, 1, sectors - 33))


def compact_super(path):
    """Trim unused tail capacity, retaining all LP extents and both metadata sets.

    Scarlet ext2 currently reads 32-bit inode sizes. A compact super also avoids
    mke2fs 1.47.3's >2 GiB population bug without changing Android's logical
    partition layout or second-stage fstab. This read-only fixture is not OTA
    capable: dynamic groups retain only their existing allocation.
    """
    with path.open("r+b") as f:
        f.seek(4096)
        geometry = f.read(4096)
        maximum, slots, block = struct.unpack_from("<III", geometry, 40)
        if (maximum, slots, block) != (65536, 3, 4096):
            raise ValueError("unexpected pinned super geometry")
        records = []
        end = 0
        for index in range(slots * 2):
            offset = 12288 + index * maximum
            f.seek(offset)
            header = bytearray(f.read(256))
            header_hash = bytes(header[12:44])
            header[12:44] = bytes(32)
            if header[:4] != b"0PLA" or hashlib.sha256(header).digest() != header_hash:
                raise ValueError("invalid LP header checksum")
            size = struct.unpack_from("<I", header, 44)[0]
            if size > maximum - 256:
                raise ValueError("invalid LP table size")
            tables = bytearray(f.read(size))
            if hashlib.sha256(tables).digest() != header[48:80]:
                raise ValueError("invalid LP tables checksum")
            po, pn, ps = struct.unpack_from("<III", header, 80)
            eo, en, es = struct.unpack_from("<III", header, 92)
            go, gn, gs = struct.unpack_from("<III", header, 104)
            bo, bn, bs = struct.unpack_from("<III", header, 116)
            if (ps, es, gs, bs, bn) != (52, 24, 48, 64, 1):
                raise ValueError("unexpected LP layout")
            extents = []
            for i in range(en):
                count, kind, first, device = struct.unpack_from("<QIQI", tables, eo + i * es)
                if kind != 0 or device != 0:
                    raise ValueError("expected linear extents on super")
                extents.append(count * 512)
                end = max(end, (first + count) * 512)
            usage = [0] * gn
            for i in range(pn):
                first, count, group = struct.unpack_from("<III", tables, po + i * ps + 40)
                if first + count > en or group >= gn:
                    raise ValueError("invalid LP partition extent/group")
                usage[group] += sum(extents[first:first + count])
            for i in range(gn):
                if struct.unpack_from("<Q", tables, go + i * gs + 40)[0]:
                    struct.pack_into("<Q", tables, go + i * gs + 40, max(MIB, align(usage[i], MIB)))
            records.append((offset, header, tables, bo))
        length = align(end + 4 * MIB, MIB)
        if length > path.stat().st_size:
            raise ValueError("super extents exceed device")
        for offset, header, tables, bo in records:
            struct.pack_into("<Q", tables, bo + 16, length)
            header[48:80] = hashlib.sha256(tables).digest()
            header[12:44] = hashlib.sha256(header).digest()
            f.seek(offset)
            f.write(header)
            f.write(tables)
        f.truncate(length)


def system_disk(assets):
    compact_super(assets / "super.raw")
    partitions = [("super", assets / "super.raw")]
    # first_stage_mount also initializes these recovery entries even though
    # crosvm loaded the kernel/ramdisk directly.
    partitions.extend((name + "_a", assets / (name + ".img"))
                      for name in ["boot", "init_boot", "vendor_boot"])
    gpt_image(assets / "system-gpt.img", partitions)


def make_ramdisk(assets):
    boot = (assets / "boot.img").read_bytes()
    generic = (assets / "init_boot.img").read_bytes()
    vendor = (assets / "vendor_boot.img").read_bytes()
    for data in [boot, generic]:
        if data[:8] != b"ANDROID!" or struct.unpack_from("<I", data, 40)[0] != 4:
            raise ValueError("expected Android boot v4")
    kernel_size = struct.unpack_from("<I", boot, 8)[0]
    kernel = boot[4096:4096 + kernel_size]
    if len(kernel) != kernel_size or kernel[56:60] != b"ARM\x64":
        raise ValueError("expected raw AArch64 Image")
    (assets / "Image").write_bytes(kernel)
    if vendor[:8] != b"VNDRBOOT" or struct.unpack_from("<I", vendor, 8)[0] != 4:
        raise ValueError("expected vendor boot v4")
    page = struct.unpack_from("<I", vendor, 12)[0]
    size = struct.unpack_from("<I", vendor, 24)[0]
    offset = align(struct.unpack_from("<I", vendor, 2096)[0], page)
    entries = android.ramdisk_entries(android.legacy_lz4(vendor[offset:offset + size]))
    size = struct.unpack_from("<I", generic, 12)[0]
    entries.update(android.ramdisk_entries(android.legacy_lz4(generic[4096:4096 + size])))
    # Retain logical/slot selection and file encryption; disable AVB for the
    # patched read-only fixture partitions.
    patched = 0
    for name, (mode, data) in list(entries.items()):
        if "fstab." not in name:
            continue
        lines = []
        for line in data.decode().splitlines():
            fields = line.split()
            if fields and not line.startswith("#") and len(fields) == 5:
                fields[4] = ",".join(x for x in fields[4].split(",") if not x.startswith("avb"))
                line = " ".join(fields)
            lines.append(line)
        entries[name] = (mode, ("\n".join(lines) + "\n").encode())
        patched += 1
    if not patched:
        raise ValueError("missing Cuttlefish fstab")
    bootconfig = {
        "androidboot.hardware": "cutf_cvm", "androidboot.fstab_suffix": "cf.ext4.cts",
        "androidboot.slot_suffix": "_a", "androidboot.force_normal_boot": "1",
        "androidboot.boot_devices": "10000.pci", "androidboot.console": "hvc0",
        "androidboot.serialconsole": "1", "androidboot.selinux": "permissive",
        "androidboot.serialno": "SCARLET-CF", "androidboot.hw_timeout_multiplier": "50",
        "androidboot.setupwizard_mode": "DISABLED", "androidboot.enable_bootanimation": "0",
        "androidboot.vendor.apex.com.android.hardware.keymint": "com.android.hardware.keymint.rust_nonsecure",
        "androidboot.vendor.apex.com.android.hardware.gatekeeper": "com.android.hardware.gatekeeper.nonsecure",
        "androidboot.vendor.apex.com.android.hardware.graphics.composer": "com.android.hardware.graphics.composer.ranchu",
        "androidboot.vendor.apex.com.google.emulated.camera.provider.hal": "com.google.emulated.camera.provider.hal",
        "androidboot.vendor.apex.com.android.hardware.strongbox": "none",
        "androidboot.vendor.apex.com.android.hardware.weaver": "none",
        # These optional peripherals require host serial/control services that
        # this VM does not provide. Excluding their APEX also omits their VINTF
        # declarations/feature XML, instead of repeatedly starting broken HALs.
        "androidboot.vendor.apex.com.google.cf.bt": "none",
        "androidboot.vendor.apex.com.android.hardware.uwb": "none",
        "androidboot.vendor.apex.com.android.hardware.threadnetwork": "none",
        "androidboot.vendor.apex.com.google.cf.oemlock": "none",
        "androidboot.vendor.apex.com.google.cf.nfc": "none",
        "androidboot.vendor.apex.com.google.cf.light": "none",
        "androidboot.vendor.apex.com.google.cf.rild": "none",
    }
    # This uses a real Linux bootconfig trailer, avoiding kernel command-line
    # truncation and the earlier "no bootconfig found" warning.
    config = "".join(f'{k} = "{v}"\n' for k, v in bootconfig.items()).encode()
    config += b"\0" * (-len(config) % 4)
    with (assets / "initramfs.cpio").open("xb") as out:
        for inode, (name, (mode, data)) in enumerate(entries.items(), 1):
            android.smoke.archive_entry(out, name, mode, data, inode)
        android.smoke.archive_entry(out, "TRAILER!!!", 0, b"", len(entries) + 1)
        out.write(b"\0" * (-out.tell() % 512))
        out.write(config)
        out.write(struct.pack("<II", len(config), sum(config)))
        out.write(b"#BOOTCONFIG\n")
    (assets / "bootconfig.json").write_text(json.dumps(bootconfig, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image-zip", type=Path, required=True)
    parser.add_argument("--base-staging", type=Path, required=True, help="GPU-2D enabled crosvm staging")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error("output already exists; use a new directory to preserve writable disks")
    if not shutil.which("mkfs.ext4"):
        parser.error("mkfs.ext4 is required")
    if args.image_zip.stat().st_size != ARCHIVE_SIZE:
        parser.error("unexpected Cuttlefish archive size")
    with args.image_zip.open("rb") as f:
        if hashlib.file_digest(f, "sha256").hexdigest() != ARCHIVE_SHA256:
            parser.error("archive does not match official build 16373615")
    assets = output / "android-assets"
    assets.mkdir(parents=True)
    with zipfile.ZipFile(args.image_zip) as archive:
        for name in ["boot.img", "init_boot.img", "vendor_boot.img", "android-info.txt"]:
            with archive.open(name) as src, (assets / name).open("xb") as dst:
                shutil.copyfileobj(src, dst)
        print("Expanding and patching private super image", flush=True)
        with archive.open("super.img") as src:
            unsparse(src, assets / "super.raw")
    make_ramdisk(assets)
    patch_vendor(assets / "super.raw")
    system_disk(assets)
    for name, size in [("metadata", 64), ("userdata", 1536), ("misc", 4)]:
        path = assets / f"{name}.img"
        with path.open("xb") as f:
            f.truncate(size * MIB)
        if name != "misc":
            subprocess.run(["mkfs.ext4", "-F", "-O", "^orphan_file", str(path)], check=True)
    gpt_image(assets / "writable-gpt.img", [(n, assets / f"{n}.img") for n in ["metadata", "userdata", "misc"]])
    # An existing profile can supply crosvm without copying its Android disks.
    base = Path(tempfile.mkdtemp(prefix=".cuttlefish-base-", dir=output.parent)) / "staging"
    shutil.copytree(args.base_staging, base, symlinks=True,
                    ignore=lambda path, names: ["guest"] if Path(path).name == "linux-aarch64" else [])
    command = [sys.executable, str(SOURCE / "prepare-os.py"), "--no-activate",
               "--base-staging", str(base), "--output", str(output),
               "--image", str(assets / "Image"), "--initrd", str(assets / "initramfs.cpio"),
               "--mem", "2048", "--gpu", "backend=2d,width=800,height=600",
               "--params", "earlycon=uart8250,mmio,0x3f8 console=hvc0 rdinit=/init bootconfig "
               "nokaslr loglevel=4 panic=-1 printk.devkmsg=on audit=0 8250.nr_uarts=1 "
               "binder.impl=rust cma=0 firmware_class.path=/vendor/etc/ loop.max_part=7",
               "--serial", "type=stdout,hardware=serial,num=1,console=false,stdin=false"]
    for number in range(1, 4):
        command += ["--serial", f"type=stdout,hardware=virtio-console,num={number},console={'true' if number == 1 else 'false'},stdin=false"]
    command += ["--read-only-disk", str(assets / "system-gpt.img"), "--disk", str(assets / "writable-gpt.img")]
    try:
        subprocess.run(command, check=True)
    finally:
        shutil.rmtree(base.parent)
    (output / "source.json").write_text(json.dumps({"build": BUILD, "target": TARGET,
        "archive_md5": ARCHIVE_MD5, "archive_sha256": ARCHIVE_SHA256,
        "android": 17, "renderer": "ANGLE + SwiftShader (pastel)",
        "success_marker": "SCARLET_ANDROID_BOOT_OK", "verified_boot": False,
        "source": f"https://ci.android.com/builds/submitted/{BUILD}/{TARGET}/latest"}, indent=2) + "\n")
    print(f"Private Cuttlefish profile ready: {output}")


if __name__ == "__main__":
    main()
