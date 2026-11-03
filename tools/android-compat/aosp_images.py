"""Preserve the complete pinned AOSP userspace images for native Scarlet boot.

This is a new input path, independent of the selected-service fixture. Image
contents, packaged APEXes, init files, properties, APKs and inode metadata are
not rewritten. Preparing inputs does not certify that Scarlet can boot them.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import zipfile

from art_inputs import SDK_SHA256, SDK_URL, sha256

MANIFEST = "aosp-inputs.json"
REQUIRED = {
    "ramdisk.img", "system.img", "vendor.img", "userdata.img",
    "encryptionkey.img", "VerifiedBootParams.textproto", "source.properties",
}


def copy_sparse(source, destination, size):
    """Keep the raw image byte-for-byte while avoiding allocated zero runs."""
    digest = hashlib.sha256()
    count = 0
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("xb") as target:
        while chunk := source.read(1024 * 1024):
            count += len(chunk)
            if count > size:
                raise ValueError("archive member exceeds its declared size")
            digest.update(chunk)
            if chunk == bytes(len(chunk)):
                target.seek(len(chunk), 1)
            else:
                target.write(chunk)
        if count != size:
            raise ValueError("truncated archive member")
        target.truncate(count)
    destination.chmod(0o444)
    return {"size": count, "sha256": digest.hexdigest()}


def member_path(name):
    prefix = "arm64-v8a/"
    if not name.startswith(prefix):
        raise ValueError("unexpected SDK member prefix")
    relative = name[len(prefix):]
    path = PurePosixPath(relative)
    if (not relative or path.is_absolute() or ".." in path.parts
            or str(path) != relative or any(c in relative for c in "\0\r\n")):
        raise ValueError("invalid SDK member path")
    return relative


def verify(output):
    metadata = json.loads((output / MANIFEST).read_text())
    if (metadata.get("kind") != "scarlet-aosp-image-inputs-v1"
            or metadata.get("sdk_sha256") != SDK_SHA256
            or metadata.get("result") != "PREPARED"
            or metadata.get("payload_changes") != []):
        raise ValueError("incomplete or unrecognized AOSP inputs")
    files = metadata["files"]
    if not REQUIRED <= files.keys():
        raise ValueError("missing AOSP boot image or metadata")
    actual = {str(p.relative_to(output)) for p in output.rglob("*")
              if p.is_file() and p != output / MANIFEST}
    if actual != set(files):
        raise ValueError("unexpected or missing AOSP input file")
    if any(p.is_symlink() for p in output.rglob("*")):
        raise ValueError("image inputs must not contain host symlinks")
    for name, record in files.items():
        member_path("arm64-v8a/" + name)
        path = output / name
        if path.stat().st_size != record["size"] or sha256(path) != record["sha256"]:
            raise ValueError("changed AOSP input: " + name)
    return metadata


def prepare(archive, output):
    if sha256(archive) != SDK_SHA256:
        raise ValueError("SDK archive does not match the pinned AOSP image")
    output.mkdir(parents=True, exist_ok=False)
    (output / MANIFEST).write_text('{"result":"PREPARING"}\n')
    files = {}
    with zipfile.ZipFile(archive) as sdk:
        for entry in sdk.infolist():
            if entry.is_dir():
                continue
            name = member_path(entry.filename)
            # Scarlet is the kernel. Keep all userspace input, but do not stage
            # the SDK's Linux kernel as an executable boot option.
            if name == "kernel-ranchu":
                continue
            if name in files:
                raise ValueError("duplicate SDK member")
            with sdk.open(entry) as source:
                files[name] = copy_sparse(source, output / name, entry.file_size)
    if not REQUIRED <= files.keys():
        raise ValueError("incomplete SDK userspace image set")
    metadata = {
        "kind": "scarlet-aosp-image-inputs-v1", "result": "PREPARED",
        "sdk_url": SDK_URL, "sdk_sha256": SDK_SHA256,
        "files": files, "payload_changes": [],
        "boot_validation": "not run",
    }
    (output / MANIFEST).write_text(json.dumps(metadata, indent=2) + "\n")
    verify(output)
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--sdk-image", type=Path)
    mode.add_argument("--verify", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    metadata = verify(args.output) if args.verify else prepare(args.sdk_image, args.output)
    print(f"Verified {len(metadata['files'])} unchanged AOSP userspace inputs; boot not asserted")


if __name__ == "__main__":
    main()
