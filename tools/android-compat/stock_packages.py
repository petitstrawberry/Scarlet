"""Complete generic AOSP trees and their platform libraries from pinned images.

This selects HALs only through explicit manager inputs. Guest symlinks stay in
the manifest until staging, and source inode identities are restored in the
generated disk rather than chowning files on the host.
"""
import argparse
import json
import os
import posixpath
import shutil
import subprocess
import tempfile
from pathlib import Path
import re
import stat

from aosp_images import verify as verify_images
from aosp_inventory import Image, extent_hash, parse_rc
from art_inputs import SDK_SHA256, quote_debugfs, sha256
from stock_art import PARTITIONS

MANIFEST = "stock-packages.json"
TREES = ("app", "priv-app", "overlay", "framework", "lib64", "media", "usr",
         "etc/permissions", "etc/sysconfig", "etc/compatconfig")
GENERIC_TREES = {"system": ("bin", "etc/init"), "system_ext": ("bin", "etc/init")}
INIT_SLOTS = {"system": "scarlet/boot/init.d/system",
              "system_ext": "scarlet/boot/init.d/system_ext"}
MANAGER_INIT_IMPORTS = ["/system/etc/init/hw/init.rc",
                        "/scarlet/boot/init.d/system",
                        "/scarlet/boot/init.d/system_ext"]

# These are declarations in stock RC files whose unavailable providers are
# deliberately outside the generic system/system_ext bootstrap. Unknown names
# are errors; this is not a general allow-missing policy.
OPTIONAL_SERVICE_REFERENCES = {
    "audio-hal-2-0", "audio_proxy_service", "vendor.audio-hal",
    "vendor.audio-hal-2-0", "vendor.audio-hal-4-0-msd",
    "vndservicemanager", "zygote_secondary", "qemu-adb-setup",
}
DYNAMIC_SERVICE_REFERENCES = {
    "llkd-${ro.debuggable:-0}": {"llkd-0", "llkd-1"},
}
OPTIONAL_SERVICE_EXECUTABLES = {
    ("boringssl_self_test32", "/system/bin/boringssl_self_test32"),
    ("boringssl_self_test_apex32", "/apex/com.android.conscrypt/bin/boringssl_self_test32"),
}
OPTIONAL_STOCK_IMPORTS = {
    "/init.${ro.hardware}.rc",
    "/vendor/etc/init/hw/init.${ro.hardware}.rc",
}


_SAFE_STOCK_PATH = re.compile(r"[A-Za-z0-9_./+@\[\]-]+")


def _stock_relative_path(name):
    """Validate package paths, including the stock toybox '[' alias."""
    if (not isinstance(name, str) or not name or name.startswith("/") or
            posixpath.normpath(name) != name or name in (".", "..") or
            name.startswith("../") or not _SAFE_STOCK_PATH.fullmatch(name)):
        raise ValueError("invalid stock package path")
    return name


def _stock_guest_alias(source, target):
    _stock_relative_path(source)
    if not target or not _SAFE_STOCK_PATH.fullmatch(target):
        raise ValueError("invalid stock package alias")
    resolved = posixpath.normpath(posixpath.join("/", posixpath.dirname(source), target))
    if not resolved.startswith(("/system/", "/system_ext/", "/product/", "/vendor/", "/apex/")):
        raise ValueError("escaping stock package alias")
    return target


def package_trees(partition):
    return TREES + GENERIC_TREES.get(partition, ())


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
                _stock_relative_path(name)
            yield name, {"mode": int(mode, 8), "uid": int(uid), "gid": int(gid),
                         "size": int(size or 0)}


def seed_objects(existing, output, metadata=None):
    """Hardlink a verified immutable object store into a new preparation."""
    metadata = verify(existing) if metadata is None else metadata
    objects = output / "objects"
    objects.mkdir()
    for source in sorted((existing / "objects").iterdir()):
        destination = objects / source.name
        try:
            os.link(source, destination)
        except OSError as error:
            raise ValueError("reused stock objects must be on the same filesystem") from error
    if {path.name for path in objects.iterdir()} != set(metadata["nodes"][name]["sha256"]
                                                         for name, record in metadata["nodes"].items()
                                                         if stat.S_ISREG(record["mode"])):
        raise ValueError("reused stock object inventory changed")


def prepare(images, partitions, output, debugfs="debugfs", reuse_objects=None):
    inputs = verify_images(images)
    reusable = verify(reuse_objects) if reuse_objects is not None else None
    output.mkdir(parents=True, exist_ok=False)
    if reusable is not None:
        seed_objects(reuse_objects, output, reusable)
    metadata = {"kind": "scarlet-stock-packages-v1", "sdk_sha256": inputs["sdk_sha256"],
                "result": "PREPARING", "trees": [], "partitions": {}, "nodes": {}}

    def visit(image, original, destination):
        _stock_relative_path(destination)
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
                record["target"] = _stock_guest_alias(relative, match[1])
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
        for tree in package_trees(partition):
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
        _stock_relative_path(name)
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
            _stock_guest_alias(name, record["target"])
        elif not stat.S_ISDIR(record["mode"]):
            raise ValueError("unsupported stock package inode")
    if {p.name for p in (inputs / "objects").iterdir()} != objects:
        raise ValueError("stock package object inventory changed")
    return metadata


def _override_digest(overrides, name):
    if name not in overrides:
        return None
    record = overrides[name]
    if (not isinstance(record, dict) or set(record) != {"sha256", "proofs"} or
            not re.fullmatch(r"[0-9a-f]{64}", record.get("sha256", "")) or
            not isinstance(record.get("proofs"), list) or not record["proofs"] or
            len(set(record["proofs"])) != len(record["proofs"])):
        raise ValueError("invalid package override proof")
    file_proof = re.compile(r"host_service_inputs\.[a-z0-9_]+\.files|vintf_inputs\.generated_sha256|egl_inputs\.files")
    exact_proof = {"vold_subdirs_inputs.binary_sha256", "shared_memory_init.configured_sha256"}
    if any(not isinstance(proof, str) or not (file_proof.fullmatch(proof) or proof in exact_proof)
           for proof in record["proofs"]):
        raise ValueError("unrecognized package override proof")
    return record["sha256"]


def verify_overrides(overrides):
    if not isinstance(overrides, dict):
        raise ValueError("package overrides must be proof records")
    for name in overrides:
        _stock_relative_path(name)
        _override_digest(overrides, name)
    return overrides


def authenticated_package_overrides(metadata):
    """Return only staged digests named by explicit provider proof fields."""
    result = {}

    def add(name, digest, proof):
        if not isinstance(name, str) or not re.fullmatch(r"[0-9a-f]{64}", digest or ""):
            raise ValueError("invalid authenticated package override")
        record = result.setdefault(name, {"sha256": digest, "proofs": []})
        if record["sha256"] != digest:
            raise ValueError("conflicting authenticated package overrides: " + name)
        if proof not in record["proofs"]:
            record["proofs"].append(proof)

    for service, record in metadata.get("host_service_inputs", {}).items():
        for name, digest in record.get("files", {}).items():
            add(name, digest, f"host_service_inputs.{service}.files")
    vold = metadata.get("vold_subdirs_inputs")
    if vold:
        add("system/bin/vold_prepare_subdirs", vold.get("binary_sha256"),
            "vold_subdirs_inputs.binary_sha256")
    memory = metadata.get("shared_memory_init")
    if memory:
        add(memory.get("source"), memory.get("configured_sha256"),
            "shared_memory_init.configured_sha256")
    for name, digest in metadata.get("vintf_inputs", {}).get("generated_sha256", {}).items():
        add(name, digest, "vintf_inputs.generated_sha256")
    for name, digest in metadata.get("egl_inputs", {}).get("files", {}).items():
        add(name, digest, "egl_inputs.files")
    for record in result.values():
        record["proofs"].sort()
    return result


def validate_host_service_proofs(metadata):
    """Run the existing manager proof gate and retain stale fields in errors."""
    from host_services import validate_staged
    try:
        validate_staged(metadata)
    except ValueError as error:
        netd = metadata.get("host_service_inputs", {}).get("netd", {})
        fields = ", ".join(f"{key}={netd.get(key)!r}" for key in
                           ("firewall_implementation", "packet_filtering", "bpf_loader")
                           if key in netd)
        suffix = ": " + fields if fields else ""
        raise ValueError(f"stale or incomplete host service proof{suffix}; {error}") from error


def stage(inputs, root, overrides=None):
    """Check the prepared runtime, leaving file materialization to the disk writer."""
    metadata = verify(inputs)
    overrides = verify_overrides(overrides or {})
    for name, record in metadata["nodes"].items():
        path = root / name
        if any(parent.is_symlink() for parent in path.parents if root in parent.parents):
            raise ValueError("stock package alias ancestor: " + name)
        mode = record["mode"]
        if path.is_symlink():
            canonical = lambda link: posixpath.normpath(posixpath.join("/", str(Path(name).parent), link))
            if not stat.S_ISLNK(mode) or canonical(str(path.readlink())) != canonical(record["target"]):
                raise ValueError("stock package alias conflict: " + name)
        elif path.exists() or path.is_symlink():
            expected = _override_digest(overrides, name) if stat.S_ISREG(mode) else None
            if stat.S_ISREG(mode) and (not path.is_file() or sha256(path) != (expected or record["sha256"])):
                raise ValueError("stock package content conflict: " + name)
            if ((stat.S_ISDIR(mode) and (path.is_symlink() or not path.is_dir())) or
                    stat.S_ISLNK(mode)):
                raise ValueError("stock package node conflict: " + name)
    return metadata


def install_image(inputs, image_path, root, debugfs="debugfs", overrides=None):
    """Write the complete package trees into a fresh, offline guest disk.

    No host pathname represents a guest filename: case-sensitive files and
    aliases therefore survive even on a case-insensitive host filesystem.
    """
    from seed_image import inode_metadata
    metadata = verify(inputs)
    overrides = verify_overrides(overrides or {})
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
                    if sha256(current) != (_override_digest(overrides, name) or record["sha256"]):
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
                if _override_digest(overrides, name):
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
                if sha256(current) != (_override_digest(overrides, name) or record["sha256"]):
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


def _node_bytes(inputs, record):
    if not stat.S_ISREG(record["mode"]):
        raise ValueError("stock init config is not regular")
    path = inputs / "objects" / record["sha256"]
    data = path.read_bytes()
    if len(data) != record["size"] or sha256(path) != record["sha256"]:
        raise ValueError("changed stock init object")
    return data


def _service_sections(text, source):
    try:
        sections = parse_rc(text)
    except (UnicodeDecodeError, ValueError) as error:
        raise ValueError(f"invalid stock init config {source}: {error}") from error
    result = []
    for section in sections:
        if section["header"][0] == "service":
            result.append((section["header"][1], section["header"][2],
                           {item["args"][0] for item in section["commands"]}))
    return result


def _expand_boot_path(value, properties):
    def replace(match):
        return properties.get(match.group(1), match.group(0))
    return re.sub(r"\$\{([A-Za-z0-9_.-]+)\}", replace, value)


def _canonical_target(name, target):
    if target.startswith("/"):
        return posixpath.normpath(target)
    return posixpath.normpath(posixpath.join("/", posixpath.dirname(name), target))


def _init_sources(nodes, partition):
    prefix = partition + "/etc/init/"
    result = {}
    for name, record in nodes.items():
        if (name.startswith(prefix) and "/" not in name[len(prefix):] and
                name.endswith(".rc")):
            if not stat.S_ISREG(record["mode"]):
                raise ValueError("stock init slot is not regular: " + name)
            result[name] = record
    if not result:
        raise ValueError("missing complete generic init tree: " + partition)
    return result


def _manager_rcs(inputs, root, metadata):
    directory = root / "scarlet/boot/services"
    actual = {}
    for path in sorted(directory.glob("*.rc")) if directory.is_dir() else []:
        if path.is_symlink() or not path.is_file():
            raise ValueError("manager init slot is not regular: " + str(path.relative_to(root)))
        relative = path.relative_to(root).as_posix()
        digest = sha256(path)
        if metadata.get("files", {}).get(relative) != digest:
            raise ValueError("changed manager init input: " + relative)
        proofs = []
        for service, record in metadata.get("host_service_inputs", {}).items():
            if record.get("managed_rc_sha256") == digest:
                proofs.append(f"host_service_inputs.{service}.managed_rc_sha256")
            if record.get("files", {}).get(relative) == digest:
                proofs.append(f"host_service_inputs.{service}.files")
        if not proofs:
            raise ValueError("unverified manager init slot: " + relative)
        actual[relative] = (path.read_text(), sorted(proofs))

    expected = []
    for service, record in metadata.get("host_service_inputs", {}).items():
        digest = record.get("managed_rc_sha256")
        if digest:
            expected.append(digest)
        for name, value in record.get("files", {}).items():
            if name.endswith(".rc"):
                expected.append(value)
    actual_digests = [sha256(root / name) for name in actual]
    if sorted(expected) != sorted(actual_digests):
        raise ValueError("missing verified manager init slot")
    return actual


def _versioned_apex_configs(directory):
    candidates = [path for path in directory.glob("*rc") if path.is_file() and not path.is_symlink()]
    selected = {}
    for path in candidates:
        parts = path.name.split(".")
        if len(parts) < 2:
            continue
        suffix = parts[-1]
        if suffix == "rc":
            sdk = 0
        else:
            match = re.fullmatch(r"(\d+)rc", suffix)
            if not match:
                continue
            sdk = int(match.group(1))
        if sdk > 33:
            continue
        base = ".".join(parts[:-1])
        if base not in selected or selected[base][0] < sdk:
            selected[base] = (sdk, path)
    return [path for _, path in sorted(selected.values(), key=lambda item: item[1].name)]


def assemble_generic_init(inputs, root, metadata):
    """Build the effective generic init view without copying the stock tree.

    Stock objects remain in the package object store. Only the small RC slots
    imported by init are materialized under `/scarlet/boot/init.d`.
    """
    packages = verify(inputs)
    nodes = packages["nodes"]
    profile = metadata.get("profile", {})
    properties = profile.get("boot_properties", {})
    manager_rcs = _manager_rcs(inputs, root, metadata)
    stock = {partition: _init_sources(nodes, partition)
             for partition in ("system", "system_ext")}
    stock_services = {name: {service for service, _command, _options in
                             _service_sections(_node_bytes(inputs, record).decode(), name)}
                      for name, record in stock["system"].items()}

    replacements = {}
    additions = {}
    for name, (text, proofs) in sorted(manager_rcs.items()):
        services = {service for service, _command, _options in _service_sections(text, name)}
        if not services:
            raise ValueError("manager init slot has no service: " + name)
        hits = [source for source, source_services in stock_services.items()
                if services & source_services]
        if len(hits) > 1 or any(source in replacements for source in hits):
            raise ValueError("ambiguous stock init replacement: " + name)
        if hits:
            source = hits[0]
            replacements[source] = {"managed": name, "text": text, "proofs": proofs}
        else:
            slot = "scarlet-" + Path(name).name
            if slot in stock["system"] or slot in additions:
                raise ValueError("manager init slot collision: " + slot)
            additions[slot] = {"managed": name, "text": text, "proofs": proofs}

    manager_init = (root / "scarlet/boot/init.rc").read_text()
    imports = [section["header"][1] for section in parse_rc(manager_init)
               if section["header"][0] == "import"]
    if (imports[:len(MANAGER_INIT_IMPORTS)] != MANAGER_INIT_IMPORTS or
            any(path.startswith(("/system/etc/init/", "/system_ext/etc/init/",
                                 "/scarlet/boot/services/"))
                for path in imports[len(MANAGER_INIT_IMPORTS):])):
        raise ValueError("manager init imports are not the complete generic bootstrap")

    effective = {}
    for name, record in stock["system"].items():
        replacement = replacements.get(name)
        effective[INIT_SLOTS["system"] + "/" + Path(name).name] = (
            replacement["text"].encode() if replacement else _node_bytes(inputs, record))
    for slot, replacement in additions.items():
        effective[INIT_SLOTS["system"] + "/" + slot] = replacement["text"].encode()
    for name, record in stock["system_ext"].items():
        effective[INIT_SLOTS["system_ext"] + "/" + Path(name).name] = _node_bytes(inputs, record)

    # Recreate only the generated view, leaving every stock and manager source
    # payload untouched.
    for directory in INIT_SLOTS.values():
        target = root / directory
        if target.exists():
            shutil.rmtree(target)
        target.mkdir(parents=True)
    for name, data in effective.items():
        target = root / name
        target.write_bytes(data)
        target.chmod(0o644)

    configs = [("scarlet/boot/init.rc", manager_init, False)]
    pending = ["/system/etc/init/hw/init.rc"]
    seen = set()
    while pending:
        raw = pending.pop(0)
        guest = _expand_boot_path(raw, properties)
        source = guest.lstrip("/")
        if source in seen:
            raise ValueError("duplicate stock init import: " + guest)
        seen.add(source)
        if source in nodes and stat.S_ISREG(nodes[source]["mode"]):
            text = _node_bytes(inputs, nodes[source]).decode()
        elif (root / source).is_file() and not (root / source).is_symlink():
            text = (root / source).read_text()
        elif (raw in OPTIONAL_STOCK_IMPORTS or guest in {
                f"/init.{properties.get('ro.hardware', '')}.rc",
                f"/vendor/etc/init/hw/init.{properties.get('ro.hardware', '')}.rc"}):
            continue
        else:
            raise ValueError("missing stock init import: " + guest)
        configs.append((source, text, False))
        for section in parse_rc(text):
            if section["header"][0] == "import":
                pending.append(section["header"][1])
    for directory in INIT_SLOTS.values():
        for path in sorted((root / directory).glob("*.rc")):
            configs.append((path.relative_to(root).as_posix(), path.read_text(), False))

    for apex in sorted(root.glob("system/apex/*/etc")):
        for path in _versioned_apex_configs(apex):
            configs.append((path.relative_to(root).as_posix(), path.read_text(), True))

    services = {}
    for source, text, from_apex in configs:
        for name, command, options in _service_sections(text, source):
            if name in services:
                old_source, old_command, old_options = services[name]
                if not (from_apex and "override" in options and "updatable" in old_options):
                    raise ValueError(
                        f"duplicate init service {name}: {old_source} and {source}")
            services[name] = (source, command, options)

    def asset_exists(guest, depth=0):
        if depth > 8:
            raise ValueError("stock executable alias chain")
        relative = guest.lstrip("/")
        record = nodes.get(relative)
        if record:
            if stat.S_ISREG(record["mode"]):
                return True
            if stat.S_ISLNK(record["mode"]):
                return asset_exists(_canonical_target(relative, record["target"]), depth + 1)
            return False
        path = root / relative
        if path.exists():
            if path.is_symlink():
                return asset_exists(_canonical_target(relative, str(path.readlink())), depth + 1)
            return path.is_file()
        if guest.startswith("/apex/"):
            return asset_exists("/system" + guest, depth + 1)
        return False

    for name, (_source, command, _options) in sorted(services.items()):
        command = _expand_boot_path(command, properties)
        if "${" in command or not command.startswith("/"):
            raise ValueError(f"unverifiable init executable for {name}: {command}")
        if not asset_exists(command):
            if (name, command) not in OPTIONAL_SERVICE_EXECUTABLES:
                raise ValueError(f"missing init executable for {name}: {command}")

    for source, text, _from_apex in configs:
        for section in parse_rc(text):
            for item in section["commands"]:
                args = item["args"]
                if not args or args[0] not in {"start", "stop", "restart", "exec_start"}:
                    continue
                values = args[1:]
                if values and values[0] == "--only-if-running":
                    values = values[1:]
                if not values:
                    raise ValueError("init service reference has no target")
                target = values[0]
                if target in services:
                    continue
                expected = DYNAMIC_SERVICE_REFERENCES.get(target)
                if expected is not None and expected <= set(services):
                    continue
                if target not in OPTIONAL_SERVICE_REFERENCES:
                    raise ValueError(f"unknown init service reference {target} in {source}")

    return {
        "kind": "scarlet-stock-init-assembly-v1", "result": "ASSEMBLED",
        "imports": list(MANAGER_INIT_IMPORTS),
        "stock_slots": sum(len(values) for values in stock.values()),
        "replacements": [{"source": source, **{key: value for key, value in record.items()
                        if key != "text"}} for source, record in sorted(replacements.items())],
        "additions": [{"slot": slot, **{key: value for key, value in record.items()
                       if key != "text"}} for slot, record in sorted(additions.items())],
        "files": {name: sha256(root / name) for name in sorted(effective)},
        "service_count": len(services),
        "stock_source_payloads": "preserved in immutable package objects",
        "optional_service_executables": sorted([" ".join(item) for item in OPTIONAL_SERVICE_EXECUTABLES]),
        "optional_service_references": sorted(OPTIONAL_SERVICE_REFERENCES | set(DYNAMIC_SERVICE_REFERENCES)),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--images", type=Path, required=True)
    parser.add_argument("--partitions", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--debugfs", default="debugfs")
    parser.add_argument("--reuse-objects", type=Path,
                        help="verified prior package directory whose immutable objects are hardlinked")
    args = parser.parse_args()
    prepare(args.images, args.partitions, args.output, args.debugfs, args.reuse_objects)


if __name__ == "__main__":
    main()
