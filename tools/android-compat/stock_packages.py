"""Complete application trees and their platform libraries from pinned images.

This does not configure init, select HALs, or change APK/JAR contents. Guest
symlinks stay in the manifest until staging, and source inode identities are
restored in the generated disk rather than chowning files on the host.
"""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path
import re
import stat

from aosp_images import verify as verify_images
from aosp_inventory import Image, extent_hash
from art_inputs import SDK_SHA256, quote_debugfs, sha256
from boot_profile import guest_alias, relative_path
from stock_art import PARTITIONS

MANIFEST = "stock-packages.json"
TREES = ("app", "priv-app", "overlay", "framework", "lib64", "media", "usr",
         "etc/permissions", "etc/sysconfig", "etc/compatconfig")


def source_entries(image, path):
    for line in image.command("ls -p " + quote_debugfs(path)).decode().splitlines():
        if not line.strip():
            continue
        match = re.fullmatch(r"/(\d+)/([0-7]+)/(\d+)/(\d+)/([^/]*)/([0-9]*)/", line)
        if not match:
            raise ValueError("invalid source inode listing")
        inode, mode, uid, gid, name, size = match.groups()
        if int(inode) and name != "..":
            if name != ".":
                relative_path(name)
            yield name, {"mode": int(mode, 8), "uid": int(uid), "gid": int(gid),
                         "size": int(size or 0)}


def prepare(images, partitions, output, debugfs="debugfs"):
    inputs = verify_images(images)
    output.mkdir(parents=True, exist_ok=False)
    metadata = {"kind": "scarlet-stock-packages-v1", "sdk_sha256": inputs["sdk_sha256"],
                "result": "PREPARING", "trees": [], "partitions": {}, "nodes": {}}

    def visit(image, original, destination):
        relative_path(destination)
        for name, record in source_entries(image, original):
            relative = destination if name == "." else destination + "/" + name
            path = original if name == "." else original + "/" + name
            mode = record["mode"]
            if name == ".":
                if not stat.S_ISDIR(mode):
                    raise ValueError("source directory metadata")
                metadata["nodes"][relative] = record
            elif stat.S_ISDIR(mode):
                visit(image, path, relative)
            elif stat.S_ISREG(mode):
                file = output / "incoming-file"
                image.dump(path, file)
                if file.stat().st_size != record["size"]:
                    raise ValueError("truncated stock file: " + relative)
                record["sha256"] = sha256(file)
                object_path = output / "objects" / record["sha256"]
                object_path.parent.mkdir(exist_ok=True)
                if object_path.exists():
                    if sha256(object_path) != record["sha256"]:
                        raise ValueError("corrupt package object")
                    file.unlink()
                else:
                    file.rename(object_path)
                metadata["nodes"][relative] = record
            elif stat.S_ISLNK(mode):
                info = image.command("stat " + quote_debugfs(path)).decode()
                match = re.search(r'^Fast link dest: "([^"\n]+)"$', info, re.MULTILINE)
                if not match:
                    raise ValueError("non-inline stock symlink: " + relative)
                record["target"] = guest_alias(relative, match[1])
                metadata["nodes"][relative] = record
            else:
                raise ValueError("unexpected stock package node: " + relative)

    for partition, (sector, sectors) in PARTITIONS.items():
        path = partitions / (partition + ".ext4")
        size = sectors * 512
        expected = extent_hash(images / "system.img", (4096 + sector) * 512, size)
        if path.stat().st_size != size or sha256(path) != expected:
            raise ValueError("partition differs from pinned SDK: " + partition)
        metadata["partitions"][partition] = expected
        image = Image(path, debugfs)
        root = "/system" if partition == "system" else ""
        available = dict(image.entries(root or "/"))
        etc = dict(image.entries(root + "/etc"))
        for tree in TREES:
            names = etc if tree.startswith("etc/") else available
            if tree.rsplit("/", 1)[-1] not in names:
                continue
            original = root + "/" + tree
            destination = partition + "/" + tree
            metadata["trees"].append(destination)
            visit(image, original, destination)
    metadata["result"] = "PREPARED"
    (output / MANIFEST).write_text(json.dumps(metadata, indent=2) + "\n")
    verify(output)
    print(f"Prepared {len(metadata['nodes'])} nodes from {len(metadata['trees'])} complete trees")


def verify(inputs):
    metadata = json.loads((inputs / MANIFEST).read_text())
    if (metadata.get("kind") != "scarlet-stock-packages-v1" or
            metadata.get("sdk_sha256") != SDK_SHA256 or metadata.get("result") != "PREPARED" or
            set(metadata.get("partitions", {})) != set(PARTITIONS)):
        raise ValueError("incomplete stock package input")
    nodes = metadata["nodes"]
    if not nodes or len(nodes) > 50000:
        raise ValueError("stock package node bound")
    objects = set()
    for name, record in nodes.items():
        relative_path(name)
        if not any(name == tree or name.startswith(tree + "/") for tree in metadata["trees"]):
            raise ValueError("node outside stock package trees")
        for parent in Path(name).parents:
            if str(parent) in nodes and not stat.S_ISDIR(nodes[str(parent)]["mode"]):
                raise ValueError("stock package non-directory ancestor")
        if any(type(record[k]) is not int or record[k] < 0 for k in ("uid", "gid", "mode", "size")):
            raise ValueError("stock inode metadata")
        if stat.S_ISREG(record["mode"]):
            digest = record["sha256"]
            if not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise ValueError("stock package object name")
            path = inputs / "objects" / digest
            if path.is_symlink() or not path.is_file() or sha256(path) != digest or path.stat().st_size != record["size"]:
                raise ValueError("changed stock package file: " + name)
            objects.add(digest)
        elif stat.S_ISLNK(record["mode"]):
            guest_alias(name, record["target"])
        elif not stat.S_ISDIR(record["mode"]):
            raise ValueError("unsupported stock package inode")
    if {p.name for p in (inputs / "objects").iterdir()} != objects:
        raise ValueError("stock package object inventory changed")
    return metadata


def stage(inputs, root, overrides=None):
    """Check the prepared runtime, leaving file materialization to the disk writer."""
    metadata = verify(inputs)
    overrides = overrides or {}
    for name, record in metadata["nodes"].items():
        path = root / name
        if any(parent.is_symlink() for parent in path.parents if root in parent.parents):
            raise ValueError("stock package alias ancestor: " + name)
        mode = record["mode"]
        if path.is_symlink():
            import posixpath
            canonical = lambda link: posixpath.normpath(posixpath.join("/", str(Path(name).parent), link))
            if not stat.S_ISLNK(mode) or canonical(str(path.readlink())) != canonical(record["target"]):
                raise ValueError("stock package alias conflict: " + name)
        elif path.exists():
            if stat.S_ISREG(mode) and (not path.is_file() or sha256(path) != overrides.get(name, record["sha256"])):
                raise ValueError("stock package content conflict: " + name)
            if stat.S_ISDIR(mode) and not path.is_dir() or stat.S_ISLNK(mode):
                raise ValueError("stock package node conflict: " + name)
    return metadata


def install_image(inputs, image_path, root, debugfs="debugfs", overrides=None):
    """Write the complete package trees into a fresh, offline guest disk.

    No host pathname represents a guest filename: case-sensitive files and
    aliases therefore survive even on a case-insensitive host filesystem.
    """
    from seed_image import inode_metadata
    metadata = verify(inputs)
    overrides = overrides or {}
    existing = inode_metadata(image_path, root)
    nodes = metadata["nodes"]
    image = Image(image_path, debugfs)
    with tempfile.TemporaryDirectory(prefix="scarlet-package-install-") as directory:
        temporary = Path(directory)
        commands = []
        directories = {""}
        for name, record in existing.items():
            if stat.S_ISDIR(record["mode"]):
                directories.add(name)
        for name in sorted(nodes, key=lambda n: (len(Path(n).parts), n)):
            record = nodes[name]
            mode = record["mode"]
            for parent in reversed(Path(name).parents):
                relative = str(parent)
                if relative == "." or relative in directories:
                    continue
                if relative in existing:
                    raise ValueError("image alias/non-directory ancestor: " + relative)
                commands.append("mkdir " + quote_debugfs(root + "/" + relative))
                directories.add(relative)
            path = root + "/" + name
            previous = existing.get(name)
            if previous:
                if stat.S_IFMT(previous["mode"]) != stat.S_IFMT(mode):
                    raise ValueError("stock image inode conflict: " + name)
                if stat.S_ISREG(mode):
                    current = temporary / "existing-file"
                    image.dump(path, current)
                    if sha256(current) != overrides.get(name, record["sha256"]):
                        raise ValueError("stock image content conflict: " + name)
                    current.unlink()
                elif stat.S_ISLNK(mode):
                    # Destination validation checked the staged alias's target.
                    commands.append("rm " + quote_debugfs(path))
            if stat.S_ISDIR(mode):
                if name not in directories:
                    commands.append("mkdir " + quote_debugfs(path))
                    directories.add(name)
            elif stat.S_ISREG(mode) and not previous:
                if name in overrides:
                    raise ValueError("missing declared device override: " + name)
                commands.append("write " + quote_debugfs((inputs / "objects" / record["sha256"]).resolve()) + " " + quote_debugfs(path))
            elif stat.S_ISLNK(mode):
                commands.append("symlink " + quote_debugfs(path) + " " + quote_debugfs(record["target"]))
            for field in ("uid", "gid", "mode"):
                value = "0" + format(record[field], "o") if field == "mode" else str(record[field])
                commands.append(f"set_inode_field {quote_debugfs(path)} {field} {value}")
        script = temporary / "write.debugfs"
        script.write_text("\n".join(commands) + "\n")
        result = subprocess.run([debugfs, "-w", "-f", str(script), str(image_path)],
                                check=True, capture_output=True, text=True)
        errors = [line for line in result.stderr.splitlines() if line and not line.startswith("debugfs ")]
        if errors:
            raise ValueError("stock package disk write: " + "; ".join(errors[:5]))
        actual = inode_metadata(image_path, root)
        for name, record in nodes.items():
            expected = {k: record[k] for k in ("mode", "uid", "gid")}
            if actual.get(name) != expected:
                raise ValueError("stock inode not preserved: " + name)
            if stat.S_ISREG(record["mode"]):
                current = temporary / "verify-file"
                image.dump(root + "/" + name, current)
                if sha256(current) != overrides.get(name, record["sha256"]):
                    raise ValueError("stock image bytes not preserved: " + name)
                current.unlink()
            elif stat.S_ISLNK(record["mode"]):
                info = image.command("stat " + quote_debugfs(root + "/" + name)).decode()
                match = re.search(r'^Fast link dest: "([^"\n]+)"$', info, re.MULTILINE)
                if not match or match[1] != record["target"]:
                    raise ValueError("stock image alias not preserved: " + name)
    return {"manifest_sha256": sha256(inputs / MANIFEST), "nodes": len(nodes),
            "device_overrides": overrides,
            "contents_and_inode_metadata": "verified"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--images", type=Path, required=True)
    parser.add_argument("--partitions", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--debugfs", default="debugfs")
    args = parser.parse_args()
    prepare(args.images, args.partitions, args.output, args.debugfs)


if __name__ == "__main__":
    main()
