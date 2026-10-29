"""Pinned image metadata for stock Android property/linkerconfig preparation."""
from pathlib import Path
import json
import posixpath
import re
import shutil
import xml.etree.ElementTree as ET
import zipfile

from art_inputs import SDK_SHA256, copy_exact, sha256, verify_art_runtime

# Linear extents in slot-zero liblp metadata, relative to the super partition.
# As in extract_system, these constants apply ONLY to the hash-pinned SDK zip.
PARTITIONS = {
    "system": (2048, 1636144),
    "system_ext": (1638400, 285816),
    "product": (1925120, 565800),
    "vendor": (2492416, 177408),
}
BIONIC_ALIASES = {f"system/lib64/{name}": f"../../apex/com.android.runtime/lib64/bionic/{name}"
                  for name in ("libc.so", "libdl.so", "libdl_android.so", "libm.so")}


def bounded_alias(source, original):
    target = posixpath.normpath(posixpath.join("/", posixpath.dirname(source), original))
    if (not target.startswith(("/system/lib64/", "/apex/")) or
            not re.fullmatch(r"/[A-Za-z0-9_./+@-]+", target)):
        raise ValueError("unsupported image library alias")
    return posixpath.relpath(target, "/" + posixpath.dirname(source))


def extract_partitions(archive, output):
    if sha256(archive) != SDK_SHA256:
        raise ValueError("SDK archive does not match pinned SHA-256")
    records = {}
    with zipfile.ZipFile(archive) as sdk, sdk.open("arm64-v8a/system.img") as source:
        for name, (sector, sectors) in PARTITIONS.items():
            path = output / (name + ".ext4")
            offset, size = (4096 + sector) * 512, sectors * 512
            source.seek(offset)
            copy_exact(source, path, size)
            with path.open("rb") as check:
                check.seek(1080)
                if check.read(2) != b"\x53\xef":
                    raise ValueError(f"{name} extent is not ext4")
            records[name] = {"offset": offset, "size": size, "sha256": sha256(path)}
    return records


def apex_identity(data):
    """Read name/version from the original ApexManifest protobuf.

    Only wire types present in this pinned schema are accepted. Keep the whole
    original protobuf for linkerconfig; this reader never rewrites its policy.
    """
    offset = 0

    def varint():
        nonlocal offset
        value = 0
        for shift in range(0, 70, 7):
            if offset >= len(data):
                raise ValueError("truncated APEX protobuf varint")
            byte = data[offset]
            offset += 1
            if shift == 63 and byte > 1:
                raise ValueError("oversized APEX protobuf varint")
            value |= (byte & 127) << shift
            if not byte & 128:
                return value
        raise ValueError("oversized APEX protobuf varint")

    fields = {}
    while offset < len(data):
        tag = varint()
        if not tag >> 3:
            raise ValueError("invalid APEX protobuf field")
        if tag & 7 == 0:
            value = varint()
        elif tag & 7 == 2:
            size = varint()
            value = data[offset:offset + size]
            offset += size
            if len(value) != size:
                raise ValueError("truncated APEX protobuf field")
        else:
            raise ValueError("unsupported APEX protobuf wire type")
        fields.setdefault(tag >> 3, []).append(value)
    if (len(fields.get(1, [])) != 1 or len(fields.get(2, [])) != 1 or
            len(fields.get(5, [])) > 1 or not isinstance(fields[1][0], bytes) or
            not isinstance(fields[2][0], int)):
        raise ValueError("ambiguous APEX identity")
    name = fields[1][0].decode("utf-8")
    version_name = fields.get(5, [b""])[0]
    if not re.fullmatch(r"[A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)+", name) or not isinstance(version_name, bytes):
        raise ValueError("invalid APEX identity")
    return name, fields[2][0], version_name.decode("utf-8")


def activation_xml(records):
    root = ET.Element("apex-info-list")
    seen = set()
    for record in records:
        name, version = record["name"], record["version"]
        if name in seen:
            raise ValueError("duplicate factory APEX")
        seen.add(name)
        source = record["source"]
        module = (f"/data/apex/decompressed/{name}@{version}.decompressed.apex"
                  if source.endswith(".capex") else source)
        ET.SubElement(root, "apex-info", moduleName=name, modulePath=module,
                      preinstalledModulePath=source, versionCode=str(version),
                      versionName=record["version_name"], isFactory="true", isActive="true")
    ET.indent(root)
    return ET.tostring(root, encoding="utf-8", xml_declaration=True) + b"\n"


def merge_properties(paths):
    """Preserve init's later-partition-wins order for this image's literal files.

    No imports/expansion or derived boot properties are synthesized. Reject
    unfamiliar syntax instead of silently inventing property-service behavior.
    """
    properties = {}
    for path in paths:
        for line in Path(path).read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            name, separator, value = line.partition("=")
            name, value = name.strip(), value.strip()
            if (not separator or not re.fullmatch(r"[\w.-]+", name) or
                    "\0" in value or "${" in value):
                raise ValueError(f"unsupported stock property syntax in {path}: {line}")
            properties[name] = value
    return "".join(f"{key}={value}\n" for key, value in sorted(properties.items()))


def derive_cpu_abi_properties(properties):
    """Android 13 init/property_service.cpp property_initialize_ro_cpu_abilist.

    This is explicit userspace bootstrap, separate from literal image merging.
    Preserve already-published ro.* values, including empty values: Android's
    PropertySet cannot replace them. No host CPU or guessed ABI is introduced.
    """
    result = dict(properties)
    prefix = "ro.product.cpu.abilist"
    if result.get(prefix, ""):
        return result
    narrow, wide = "", ""
    for partition in ("product", "odm", "vendor", "system"):
        base = "ro." + partition + ".product.cpu.abilist"
        narrow, wide = result.get(base + "32", ""), result.get(base + "64", "")
        if narrow or wide:
            break
    result.setdefault(prefix, ",".join(part for part in (wide, narrow) if part))
    result.setdefault(prefix + "32", narrow)
    result.setdefault(prefix + "64", wide)
    return result


def verify_stock_inputs(stock, runtime):
    """Check the complete overlay and its binding to the original runtime."""
    verify_art_runtime(runtime)
    metadata = json.loads((stock / "stock-inputs.json").read_text())
    if (metadata.get("result") != "PREPARED" or metadata["sdk"]["sha256"] != SDK_SHA256 or
            metadata["base_manifest_sha256"] != sha256(runtime / "art-runtime.json")):
        raise ValueError("incomplete or mismatched stock inputs")
    links = metadata["symlinks"]
    if any(links.get(name) != target for name, target in BIONIC_ALIASES.items()):
        raise ValueError("missing stock Bionic aliases")
    for name, target in links.items():
        if Path(target).is_absolute() or bounded_alias(name, target) != target:
            raise ValueError("invalid stock library alias")
    files, directories = metadata["files"], set(metadata["directories"])
    for name in set(files) | directories | set(links):
        if not name or Path(name).is_absolute() or ".." in Path(name).parts or any(c in name for c in "\0\r\n"):
            raise ValueError("invalid stock input path")
    actual_files, actual_dirs, actual_links = set(), set(), {}
    for path in stock.rglob("*"):
        name = str(path.relative_to(stock))
        if path.is_symlink():
            actual_links[name] = str(path.readlink())
        elif path.is_file() and name != "stock-inputs.json":
            if name not in files or sha256(path) != files[name]:
                raise ValueError(f"changed or unrecorded stock input: {name}")
            actual_files.add(name)
        elif path.is_dir():
            actual_dirs.add(name)
    if actual_files != set(files) or actual_dirs != directories or actual_links != links:
        raise ValueError("missing or unexpected stock files/directories/symlinks")
    return metadata


def decode_logd(packet):
    # Android liblog's packed logdw header: id:u8, tid:u16, sec:u32, nsec:u32.
    if len(packet) < 14:
        raise ValueError("truncated logd packet")
    priority = packet[11]
    tag, separator, message = packet[12:].partition(b"\0")
    if not separator or priority not in range(2, 9):
        raise ValueError("invalid logd text packet")
    return {"priority": priority, "tag": tag.decode(errors="replace"),
            "message": message.rstrip(b"\0").decode(errors="replace")}


def linkerconfig_succeeded(returncode, messages):
    # Android 13 --strict can log ERROR and emit a partial file yet exit zero.
    return returncode == 0 and all(message["priority"] < 6 for message in messages)


def stage_stock_inputs(stock, runtime, root):
    """Add verified stock inputs to an already copied base runtime."""
    metadata = verify_stock_inputs(stock, runtime)
    for relative in metadata["directories"]:
        (root / relative).mkdir(parents=True, exist_ok=True)
    for relative, digest in metadata["files"].items():
        destination = root / relative
        if destination.is_symlink() or (destination.exists() and sha256(destination) != digest):
            raise ValueError(f"stock input would replace a different base file: {relative}")
        shutil.copy2(stock / relative, destination)
        if sha256(destination) != digest:
            raise ValueError(f"stock input changed during staging: {relative}")
    for relative, target in metadata["symlinks"].items():
        (root / relative).symlink_to(target)
    for relative in metadata["symlinks"]:
        path = root / relative
        if not path.resolve().is_relative_to(root.resolve()) or not path.is_file():
            raise ValueError(f"missing or escaping library alias target: {relative}")
    return metadata
