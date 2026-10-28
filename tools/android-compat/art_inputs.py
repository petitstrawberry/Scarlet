"""Read-only input preparation for the pinned Android 13 ARM64 ART fixture."""
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import zipfile
import zlib

SDK_URL = "https://dl.google.com/android/repository/sys-img/android/arm64-v8a-33_r02.zip"
SDK_SHA256 = "55ba9a20d473dd5351c77342016956342c40375f3e073b65a43dc9db13ccc5c6"
# The hash above fixes both GPT and liblp metadata. This is deliberately a
# single-image extractor, not a general GPT, sparse-image or super parser.
# GPT super starts at sector 4096; its system linear extent starts at 2048
# and contains 1,636,144 sectors (liblp metadata version 10.0, slot zero).
SYSTEM_OFFSET = (4096 + 2048) * 512
SYSTEM_SIZE = 1636144 * 512


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def copy_exact(source, target, size):
    remaining = size
    with Path(target).open("xb") as dest:
        while remaining:
            chunk = source.read(min(1024 * 1024, remaining))
            if not chunk:
                raise ValueError("truncated image extent")
            dest.write(chunk)
            remaining -= len(chunk)


def extract_system(archive, target):
    if sha256(archive) != SDK_SHA256:
        raise ValueError("SDK archive does not match pinned Android 33 ARM64 revision 2 SHA-256")
    with zipfile.ZipFile(archive) as sdk:
        with sdk.open("arm64-v8a/system.img") as source:
            source.seek(SYSTEM_OFFSET)
            copy_exact(source, target, SYSTEM_SIZE)
        properties = sdk.read("arm64-v8a/source.properties").decode()
        build = sdk.read("arm64-v8a/build.prop").decode()
    with Path(target).open("rb") as stream:
        stream.seek(1080)
        if stream.read(2) != b"\x53\xef":
            raise ValueError("system extent is not ext4")
    return {"url": SDK_URL, "sha256": SDK_SHA256, "source_properties": properties,
            "build_properties": build, "system_offset": SYSTEM_OFFSET, "system_size": SYSTEM_SIZE}


def unpack_apex(archive, target):
    """Accept APEX or CAPEX; never extract archive paths to the host filesystem."""
    archive, target = Path(archive), Path(target)
    with zipfile.ZipFile(archive) as apex:
        if "original_apex" in apex.namelist():
            original = target.with_suffix(".apex")
            info = apex.getinfo("original_apex")
            if info.file_size > 512 * 1024**2:
                raise ValueError("embedded APEX exceeds 512 MiB")
            with apex.open(info) as source:
                copy_exact(source, original, info.file_size)
            return unpack_apex(original, target)
        info = apex.getinfo("apex_payload.img")
        if info.file_size > 512 * 1024**2:
            raise ValueError("APEX payload exceeds 512 MiB")
        with apex.open(info) as source:
            copy_exact(source, target, info.file_size)
    return sha256(archive)


def quote_debugfs(value):
    value = str(value)
    if any(c in value for c in "\r\n\0"):
        raise ValueError("invalid debugfs path")
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


class Ext4:
    def __init__(self, image):
        self.image = Path(image)

    def command(self, command):
        return subprocess.run(["debugfs", "-R", command, str(self.image)], check=True,
                              capture_output=True, text=True).stdout

    def entries(self, path):
        rows = self.command("ls -p " + quote_debugfs(path))
        entries = []
        for line in rows.splitlines():
            if not line.strip():
                continue
            match = re.fullmatch(r"/(\d+)/([0-7]+)/\d+/\d+/([^/]+)/([0-9]*)/", line)
            if not match:
                raise ValueError(f"unexpected debugfs listing: {line}")
            name = match[3]
            if name in (".", "..") or int(match[1]) == 0:
                continue
            entries.append((name, int(match[2], 8), int(match[4] or 0)))
        if not entries:
            raise ValueError(f"missing or empty ext4 directory: {path}")
        return entries

    def dump(self, source, target, size=None):
        target = Path(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() or target.is_symlink():
            raise ValueError(f"refusing to overwrite {target}")
        self.command(f"dump {quote_debugfs(source)} {quote_debugfs(target)}")
        # debugfs can report success when the command failed.
        if not target.is_file() or not target.stat().st_size or (size is not None and target.stat().st_size != size):
            raise ValueError(f"missing or incomplete ext4 file: {source}")
        target.chmod(0o755 if "/bin/" in str(source) else 0o644)

    def tree(self, source, target):
        for name, mode, size in self.entries(source):
            if mode & 0o170000 == 0o040000:
                self.tree(source + "/" + name, Path(target) / name)
            elif mode & 0o170000 == 0o100000:
                self.dump(source + "/" + name, Path(target) / name, size)
            elif mode & 0o170000 != 0o120000:
                raise ValueError(f"unsupported ext4 node: {source}/{name}")
            # APEX symlinks to duplicate libs are unnecessary in the fixture's
            # explicit search path. Never materialize image symlinks on the host.


def elf_needed(path):
    with Path(path).open("rb") as stream:
        header = stream.read(64)
    if len(header) < 64 or header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 183:
        raise ValueError(f"not an ARM64 ELF: {path}")
    dynamic = subprocess.run(["readelf", "-d", str(path)], check=True, capture_output=True, text=True).stdout
    names = re.findall(r"\(NEEDED\).*Shared library: \[([^\]]+)\]", dynamic)
    if any("/" in name or not re.fullmatch(r"[\w.+@-]+", name) for name in names):
        raise ValueError(f"unsupported ELF dependency name in {path}: {names}")
    return names


def validate_dex(path):
    data = Path(path).read_bytes()
    if len(data) < 112 or data[:8] != b"dex\n035\0":
        raise ValueError("expected DEX 035")
    if struct.unpack_from("<III", data, 32) != (len(data), 112, 0x12345678):
        raise ValueError("invalid DEX header")
    if hashlib.sha1(data[32:]).digest() != data[12:32] or zlib.adler32(data[12:]) != struct.unpack_from("<I", data, 8)[0]:
        raise ValueError("invalid DEX checksum")
    return sha256(path)


def verify_art_runtime(root):
    """Bind the launch to a complete, unchanged prepared runtime."""
    root = Path(root).resolve()
    metadata = json.loads((root / "art-runtime.json").read_text())
    if metadata.get("result") != "PREPARED" or metadata.get("sdk", {}).get("sha256") != SDK_SHA256:
        raise ValueError("ART runtime is not a completed pinned preparation")
    files = metadata["files"]
    required = ["apex/com.android.art/bin/dalvikvm64", "apex/com.android.art/lib64/libart.so",
                "apex/com.android.runtime/bin/linker64", "system/etc/public.libraries.txt"]
    for name in required:
        if name not in files:
            raise ValueError(f"missing ART runtime identity: {name}")
    for name, digest in files.items():
        path = root / name
        if (not name or Path(name).is_absolute() or ".." in Path(name).parts or
                not path.resolve().is_relative_to(root) or path.is_symlink()):
            raise ValueError("invalid ART manifest path")
        if sha256(path) != digest:
            raise ValueError(f"ART runtime hash mismatch: {name}")
    for path in root.rglob("*"):
        name = str(path.relative_to(root))
        if path.is_symlink() and name != "system/bin/linker64":
            raise ValueError(f"unexpected ART runtime symlink: {name}")
        if path.is_file() and not path.is_symlink() and name not in files and name not in (
                "art-runtime.json", "data/local/tmp/classes.dex"):
            raise ValueError(f"unrecorded ART runtime file: {name}")
    linker = root / "system/bin/linker64"
    if not linker.is_symlink() or linker.readlink() != Path("../../apex/com.android.runtime/bin/linker64"):
        raise ValueError("unexpected ART linker alias")
    for key in ("library_path", "boot_class_path"):
        entries = metadata[key].split(":")
        if not entries or any(not p.startswith("/") or ".." in Path(p).parts or
                              any(c in p for c in "\n\r\0\x1f") for p in entries):
            raise ValueError(f"invalid {key}")
        for entry in entries:
            path = root / entry.lstrip("/")
            if not path.exists() or not path.resolve().is_relative_to(root):
                raise ValueError(f"missing {key} entry: {entry}")
            if key == "library_path" and not path.is_dir():
                raise ValueError("library path entry is not a directory")
            if key == "boot_class_path" and entry.lstrip("/") not in files:
                raise ValueError("boot class path is not covered by runtime hashes")
    return metadata


def art_launch(metadata):
    """Keep recorded and compiled launcher arguments identical."""
    return {
        "argv": ["/apex/com.android.art/bin/dalvikvm64", "-Xint", "-Xusejit:false",
                 "-Ximage:/system/nonexistent/boot.art", "-Xnoimage-dex2oat",
                 "-Xbootclasspath:" + metadata["boot_class_path"],
                 "-Xms16m", "-Xmx128m", "-cp", "/data/local/tmp/classes.dex", "ScarletArtHello"],
        "env": ["PATH=/system/bin:/apex/com.android.art/bin", "ANDROID_ROOT=/system",
                "ANDROID_DATA=/data", "ANDROID_ART_ROOT=/apex/com.android.art",
                "ANDROID_I18N_ROOT=/apex/com.android.i18n", "ANDROID_TZDATA_ROOT=/apex/com.android.tzdata",
                "ANDROID_LOG_TAGS=*:v", "LD_LIBRARY_PATH=" + metadata["library_path"]],
    }
