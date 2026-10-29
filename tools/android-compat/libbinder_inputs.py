"""Pinned build inputs and identity checks for the stock libbinder probe."""
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import tarfile
import urllib.request

from art_inputs import sha256, verify_art_runtime

SOURCE = Path(__file__).resolve().parent
LIBRARIES = ("system/lib64/libbinder.so", "system/lib64/libutils.so", "system/lib64/liblog.so",
             "apex/com.android.art/lib64/libc++.so")


def extract_headers(data, expected_tree_sha256, target):
    """Extract only regular header files; upstream compatibility symlinks are unused."""
    members, contents, total = [], {}, 0
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            name = PurePosixPath(member.name)
            if name.is_absolute() or ".." in name.parts:
                raise ValueError("header archive path escapes destination")
            if member.isdir() or member.issym() or member.islnk():
                continue
            if not member.isfile():
                raise ValueError("unsupported header archive entry")
            total += member.size
            if member.size > 2 * 1024**2 or total > 16 * 1024**2 or len(members) >= 1000:
                raise ValueError("header archive exceeds extraction limits")
            if member.name in contents:
                raise ValueError("duplicate header archive path")
            contents[member.name] = hashlib.sha256(archive.extractfile(member).read()).hexdigest()
            members.append(member)
        # Gitiles archive bytes vary across requests; pin the complete file-content
        # map instead. Skipped symlinks are never used.
        digest = hashlib.sha256(json.dumps(contents, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        if digest != expected_tree_sha256:
            raise ValueError("libbinder header tree hash mismatch")
        target.mkdir(parents=True, exist_ok=False)
        for member in members:
            path = target / member.name
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as stream:
                stream.write(archive.extractfile(member).read())


def prepare_headers(output, pins):
    for name, pin in pins["archives"].items():
        url = (f"https://android.googlesource.com/{pin['repository']}/+archive/"
               f"{pin['revision']}/{pin['path']}.tar.gz")
        with urllib.request.urlopen(url, timeout=30) as response:
            data = response.read(8 * 1024**2 + 1)
        if len(data) > 8 * 1024**2:
            raise ValueError("header download exceeds 8 MiB")
        extract_headers(data, pin["tree_sha256"], output / name)
    return pins


def verify_libbinder_build(directory, runtime):
    metadata = json.loads((directory / "build.json").read_text())
    verify_art_runtime(runtime)
    if metadata.get("result") != "BUILT" or metadata.get("kind") != "stock-libbinder-probe-v1":
        raise ValueError("not a completed libbinder probe build")
    if metadata.get("runtime_manifest_sha256") != sha256(runtime / "art-runtime.json"):
        raise ValueError("libbinder build/runtime manifest mismatch")
    expected = {name: sha256(runtime / name) for name in LIBRARIES}
    if metadata.get("runtime_libraries") != expected:
        raise ValueError("libbinder runtime libraries changed since linking")
    if metadata.get("source_sha256") != sha256(SOURCE / "libbinder-probe.cpp") or \
            metadata.get("headers_manifest_sha256") != sha256(SOURCE / "libbinder-headers.json"):
        raise ValueError("libbinder probe source or header pins changed; rebuild")
    binary = directory / "libbinder-probe"
    if metadata.get("binary_sha256") != sha256(binary):
        raise ValueError("libbinder probe binary changed since build")
    needed = metadata.get("needed", [])
    if not {"libbinder.so", "libc++.so"}.issubset(needed) or "libc++_shared.so" in needed:
        raise ValueError("probe must link the platform Binder and C++ libraries")
    return binary
