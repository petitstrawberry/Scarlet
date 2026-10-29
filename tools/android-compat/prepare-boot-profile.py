#!/usr/bin/env python3
"""Prepare verified factory payloads and explicit manager configuration for stock init."""
import argparse
import json
from pathlib import Path
import re
import shutil
import stat
import tempfile

from art_inputs import Ext4, SDK_SHA256, elf_needed, quote_debugfs, sha256, unpack_apex
from boot_profile import MANIFEST, POLICY, guest_alias, relative_path, shield_policy, validate_profile, verify_boot_inputs
from stock_art import apex_identity, extract_partitions, verify_stock_inputs


def entries(image, source):
    info = image.command("stat " + quote_debugfs(source))
    if "Type: directory" not in info:
        raise ValueError(f"not an ext4 directory: {source}")
    result = []
    for row in image.command("ls -p " + quote_debugfs(source)).splitlines():
        if not row.strip():
            continue
        match = re.fullmatch(r"/(\d+)/([0-7]+)/\d+/\d+/([^/]*)/([0-9]*)/", row)
        if not match:
            raise ValueError(f"malformed ext4 listing: {row}")
        if int(match[1]) == 0 or match[3] in (".", ".."):
            continue
        relative_path(match[3])
        if "/" in match[3]:
            raise ValueError("invalid ext4 entry name")
        result.append((match[3], int(match[2], 8), int(match[4] or 0)))
    return result


def tree(image, source, destination, output, aliases):
    relative_path(destination)
    (output / destination).mkdir(parents=True, exist_ok=True)
    for name, mode, size in entries(image, source):
        original, relative = source.rstrip("/") + "/" + name, destination + "/" + name
        relative_path(relative)
        if stat.S_ISDIR(mode):
            tree(image, original, relative, output, aliases)
        elif stat.S_ISREG(mode):
            image.dump(original, output / relative, size)
            (output / relative).chmod(mode & 0o777)
        elif stat.S_ISLNK(mode):
            info = image.command("stat " + quote_debugfs(original))
            match = re.search(r'^Fast link dest: "([^"\n]+)"$', info, re.MULTILINE)
            if not match:
                raise ValueError(f"unsupported image link storage: {original}")
            # Retain guest links as manifest data, never as host filesystem links.
            aliases[relative] = guest_alias(relative, match[1])
        else:
            raise ValueError(f"unsupported payload node: {original}")


def prepare(archive, runtime, stock, profile_file, output):
    base = verify_stock_inputs(stock, runtime)
    profile = validate_profile(json.loads(profile_file.read_text()))
    output.mkdir(parents=True, exist_ok=False)
    manifest = output / MANIFEST
    manifest.write_text('{"result":"PREPARING"}\n')
    aliases, payloads, needed = {}, [], {}
    contexts = [name for name in base["files"] if name.endswith("_property_contexts")]
    metadata = {"kind": "scarlet-android-boot-inputs-v1", "result": "PREPARING", "sdk_sha256": SDK_SHA256,
                "runtime_manifest_sha256": sha256(runtime / "art-runtime.json"),
                "stock_manifest_sha256": sha256(stock / "stock-inputs.json"),
                "profile": profile, "profile_source_sha256": sha256(profile_file),
                "property_context_files": contexts, "replacements": {}}
    with tempfile.TemporaryDirectory(prefix="boot-inputs-", dir=output) as directory:
        temporary = Path(directory)
        metadata["partitions"] = extract_partitions(archive, temporary)
        system = Ext4(temporary / "system.ext4")
        selected = {record["name"]: record for record in base["factory_apexes"]}
        for name in profile["apex_modules"]:
            record = selected[name]
            package, payload = temporary / Path(record["source"]).name, temporary / "payload.ext4"
            system.dump(record["source"], package)
            if sha256(package) != record["package_sha256"]:
                raise ValueError("factory package differs from verified stock input")
            unpack_apex(package, payload)
            if sha256(payload) != record["payload_sha256"]:
                raise ValueError("factory APEX payload changed")
            apex = Ext4(payload)
            tree(apex, "/", "system/apex/" + name, output, aliases)
            identity = apex_identity((output / "system/apex" / name / "apex_manifest.pb").read_bytes())
            if identity != (name, record["version"], record["version_name"]):
                raise ValueError("factory manifest identity mismatch")
            payloads.append(record)
            package.unlink()
            payload.unlink()
            embedded = payload.with_suffix(".apex")
            if embedded.exists():
                embedded.unlink()
        tree(system, "/system/lib64/bootstrap", "system/lib64/bootstrap", output, aliases)
        tree(system, "/system/etc/init/hw", "system/etc/init/hw", output, aliases)
        for path in profile["init_imports"]:
            if path.startswith("/system/etc/init/hw/"):
                continue
            system.dump(path, output / path[1:])
        queue = []
        for name in ["init", "ueventd", "apexd", "servicemanager", "boringssl_self_test64", "bootstrap/linker64", "bootstrap/linkerconfig"]:
            path = output / "system/bin" / name
            if name == "ueventd":
                info = system.command('stat "/system/bin/ueventd"')
                if 'Fast link dest: "init"' not in info:
                    raise ValueError("unexpected ueventd alias")
                aliases["system/bin/ueventd"] = "init"
                continue
            system.dump("/system/bin/" + name, path)
            queue.append(path)
        available = set(base["files"]) | set(base["symlinks"]) | set(
            json.loads((runtime / "art-runtime.json").read_text())["files"])
        seen = set()
        while queue:
            binary = queue.pop()
            key = str(binary.relative_to(output))
            if key in seen:
                continue
            seen.add(key)
            needed[key] = elf_needed(binary)
            for lib in needed[key]:
                relative = "system/lib64/" + lib
                if relative in available:
                    continue
                target = output / relative
                if not target.exists():
                    system.dump("/" + relative, target)
                queue.append(target)
    configure(output, stock, profile, contexts, metadata)
    metadata.update(result="PREPARED", activation="not executed", factory_apexes=payloads, needed=needed,
                    symlinks=aliases,
                    files={str(path.relative_to(output)): sha256(path) for path in sorted(output.rglob("*"))
                           if path.is_file() and path != manifest},
                    directories=[str(path.relative_to(output)) for path in sorted(output.rglob("*")) if path.is_dir()])
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    verify_boot_inputs(output, runtime, stock)
    print(f"Prepared {len(payloads)} flattened factory APEXes; stock init boot has not been tested")


def configure(output, stock, profile, contexts, metadata):
    (output / "scarlet/boot").mkdir(parents=True, exist_ok=True)
    (output / POLICY).write_text(shield_policy(profile, [(stock / name).read_text() for name in contexts]))
    # Stock init loads product properties last. Keep the original file separately
    # and record the deliberate machine-profile replacement and exact old hash.
    product = "product/etc/build.prop"
    original = (stock / product).read_text()
    (output / "scarlet/boot/product-build.prop.original").write_text(original)
    (output / product).parent.mkdir(parents=True, exist_ok=True)
    (output / product).write_text(original + "\n# Scarlet manager boot profile\n" + "".join(
        f"{name}={value}\n" for name, value in sorted(profile["boot_properties"].items())))
    metadata["replacements"][product] = json.loads((stock / "stock-inputs.json").read_text())["files"][product]
    (output / "proc").mkdir(exist_ok=True)
    (output / "proc/bootconfig").write_text("".join(
        f'{name} = "{value}"\n' for name, value in sorted(profile["bootconfig"].items())))
    (output / "scarlet/boot/init.rc").write_text("# Explicit Scarlet device boot configuration\n" + "".join(
        "import " + path + "\n" for path in profile["init_imports"]))
    for name in ["apex", "linkerconfig", "data", "sys/fs/selinux", "system_ext/apex", "product/apex", "vendor/apex"]:
        (output / name).mkdir(parents=True, exist_ok=True)
    if "resource_groups" in profile:
        (output / "etc").mkdir(exist_ok=True)
        (output / "etc/cgroups.json").write_text(json.dumps({"Cgroups": [], "Cgroups2": {
            "Path": profile["resource_groups"]["cgroup2_root"], "Mode": "0755", "UID": "root", "GID": "root", "Controllers": [],
        }}, indent=2) + "\n")
        (output / "sys/fs/cgroup").mkdir(parents=True, exist_ok=True)


def reconfigure(inputs, runtime, stock, profile_file, output):
    metadata = verify_boot_inputs(inputs, runtime, stock)
    profile = validate_profile(json.loads(profile_file.read_text()))
    if any(profile[key] != metadata["profile"][key] for key in ("apex_modules", "init_imports")):
        raise ValueError("changed payload or imports require preparation from the pinned image")
    original_manifest = sha256(inputs / MANIFEST)
    shutil.copytree(inputs, output)
    metadata.update(profile=profile, profile_source_sha256=sha256(profile_file), reused_payload_manifest_sha256=original_manifest)
    configure(output,stock,profile,metadata["property_context_files"],metadata)
    metadata["files"] = {str(path.relative_to(output)):sha256(path) for path in sorted(output.rglob("*")) if path.is_file() and path.name != MANIFEST}
    metadata["directories"] = [str(path.relative_to(output)) for path in sorted(output.rglob("*")) if path.is_dir()]
    (output / MANIFEST).write_text(json.dumps(metadata,indent=2)+"\n")
    verify_boot_inputs(output,runtime,stock)
    print("Prepared new manager configuration using verified factory payloads; boot has not been tested")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--sdk-image", type=Path)
    source.add_argument("--reuse-payloads", type=Path)
    for flag in ["runtime-root", "stock-inputs", "profile", "output"]:
        parser.add_argument("--" + flag, type=Path, required=True)
    args = parser.parse_args()
    if args.reuse_payloads:
        reconfigure(args.reuse_payloads, args.runtime_root, args.stock_inputs, args.profile, args.output)
    else:
        prepare(args.sdk_image, args.runtime_root, args.stock_inputs, args.profile, args.output)


if __name__ == "__main__":
    main()
