#!/usr/bin/env python3
"""Prepare real ARM64 ART/Bionic from the pinned API 33 SDK image, without mounts.

Run inside nix develop. This only prepares inputs; it does not certify ART
execution. Android framework services and linker namespaces are not provided.
"""
import argparse
import json
from pathlib import Path
import shutil
import tempfile

from art_inputs import Ext4, elf_needed, extract_system, sha256, unpack_apex

APEXES = {
    "com.android.runtime": ("apex", ("lib64/bionic",), ("bin/linker64",)),
    "com.android.art": ("capex", ("lib64", "javalib"), ("bin/dalvikvm64",)),
    "com.android.i18n": ("apex", ("lib64", "javalib", "etc/icu"), ()),
    "com.android.os.statsd": ("apex", (), ("lib64/libstatssocket.so",)),
    "com.android.adbd": ("capex", (), ("lib64/libadbconnection_client.so",)),
    "com.android.tzdata": ("apex", ("etc/tz",), ()),
}
LIBRARY_DIRS = [
    "apex/com.android.art/lib64", "apex/com.android.runtime/lib64/bionic",
    "apex/com.android.i18n/lib64", "apex/com.android.os.statsd/lib64",
    "apex/com.android.adbd/lib64", "system/lib64",
]
BOOT_JARS = [
    "apex/com.android.art/javalib/core-oj.jar",
    "apex/com.android.art/javalib/core-libart.jar",
    "apex/com.android.i18n/javalib/core-icu4j.jar",
]


def prepare(archive, output):
    output.mkdir(parents=True, exist_ok=False)
    metadata = {"result": "PREPARING", "execution": "not tested"}
    manifest = output / "art-runtime.json"
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    with tempfile.TemporaryDirectory(prefix="art-inputs-", dir=output) as temporary:
        temporary = Path(temporary)
        system_image = temporary / "system.ext4"
        metadata["sdk"] = extract_system(archive, system_image)
        system = Ext4(system_image)
        metadata["apexes"] = {}
        for name, (extension, trees, files) in APEXES.items():
            package = temporary / f"{name}.{extension}"
            system.dump(f"/system/apex/{package.name}", package)
            payload = temporary / f"{name}.ext4"
            original_hash = unpack_apex(package, payload)
            metadata["apexes"][name] = {"source": f"/system/apex/{package.name}",
                "package_sha256": sha256(package), "apex_sha256": original_hash,
                "payload_sha256": sha256(payload)}
            image = Ext4(payload)
            destination = output / "apex" / name
            for tree in trees:
                image.tree("/" + tree, destination / tree)
            for file in files:
                image.dump("/" + file, destination / file)
        # libartpalette loads this by name, rather than via DT_NEEDED.
        system.dump("/system/lib64/libartpalette-system.so", output / "system/lib64/libartpalette-system.so")
        search = [output / directory for directory in LIBRARY_DIRS]
        queue = [output / "apex/com.android.art/bin/dalvikvm64",
                 output / "apex/com.android.runtime/bin/linker64"]
        queue += sorted(output.rglob("*.so"))
        # libnativeloader requires the real public-library list and preloads its
        # entries. Preserve the image policy instead of inventing an empty list.
        public = output / "system/etc/public.libraries.txt"
        system.dump("/system/etc/public.libraries.txt", public)
        for line in public.read_text().splitlines():
            fields = line.split("#", 1)[0].split()
            if not fields or "nopreload" in fields or "32" in fields:
                continue
            name = fields[0]
            if "/" in name or not name.endswith(".so"):
                raise ValueError("unexpected public library name")
            found = next((directory / name for directory in search if (directory / name).is_file()), None)
            if found is None:
                found = output / "system/lib64" / name
                system.dump("/system/lib64/" + name, found)
            queue.append(found)
        seen, dependencies = set(), {}
        while queue:
            binary = queue.pop(0)
            if binary in seen:
                continue
            seen.add(binary)
            names = elf_needed(binary)
            dependencies[str(binary.relative_to(output))] = names
            for name in names:
                candidates = [directory / name for directory in search]
                found = next((path for path in candidates if path.is_file()), None)
                if found is None:
                    found = output / "system/lib64" / name
                    system.dump("/system/lib64/" + name, found)
                queue.append(found)
        metadata["needed"] = dependencies
        system.dump("/system/build.prop", output / "system/build.prop")
    linker = output / "system/bin/linker64"
    linker.parent.mkdir(parents=True)
    linker.symlink_to("../../apex/com.android.runtime/bin/linker64")
    for directory in ["dev", "proc", "sys", "tmp", "data/dalvik-cache/arm64", "data/local/tmp"]:
        (output / directory).mkdir(parents=True, exist_ok=True)
    metadata.update({"result": "PREPARED", "library_path": ":".join("/" + d for d in LIBRARY_DIRS),
        "boot_class_path": ":".join("/" + jar for jar in BOOT_JARS),
        "files": {str(p.relative_to(output)): sha256(p) for p in sorted(output.rglob("*"))
                  if p.is_file() and not p.is_symlink() and p != manifest}})
    for jar in BOOT_JARS:
        if not (output / jar).is_file():
            raise ValueError(f"missing boot jar: {jar}")
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-image", type=Path, required=True, help="arm64-v8a-33_r02.zip (SHA-256 pinned)")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    for tool in ("debugfs", "readelf"):
        if not shutil.which(tool):
            parser.error(f"{tool} is required; use nix develop")
    prepare(args.sdk_image.resolve(), args.output.resolve())
    print(args.output.resolve())


if __name__ == "__main__":
    main()
