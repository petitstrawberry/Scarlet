#!/usr/bin/env python3
"""Prepare stock property/APEX additions for the pinned ARM64 ART runtime.

Run in nix develop. Output is an overlay input tree, NOT a booted Android root
or a generated namespace policy. Execute property-bootstrap and the image's
unmodified linkerconfig in the target root to generate the actual state.
"""
import argparse
import json
from pathlib import Path
import re
import posixpath
import shutil
import tempfile

from art_inputs import Ext4, elf_needed, sha256, unpack_apex, verify_art_runtime
from stock_art import BIONIC_ALIASES, activation_xml, apex_identity, bounded_alias, derive_cpu_abi_properties, extract_partitions, merge_properties


def stock_libraries(image, source, target, output, runtime, links):
    for name, mode, size in image.entries(source):
        original, relative = source + "/" + name, target + "/" + name
        path = output / relative
        if mode & 0o170000 == 0o040000:
            path.mkdir(parents=True, exist_ok=True)
            stock_libraries(image, original, relative, output, runtime, links)
        elif mode & 0o170000 == 0o100000:
            if not (runtime / relative).is_file():
                image.dump(original, path, size)
        elif mode & 0o170000 == 0o120000:
            stat = image.command('stat "' + original + '"')
            match = re.search(r'^Fast link dest: "([^"]+)"$', stat, re.MULTILINE)
            if not match:
                raise ValueError("unsupported image symlink storage")
            alias = bounded_alias(relative, match[1])
            path.parent.mkdir(parents=True, exist_ok=True)
            path.symlink_to(alias)
            links[relative] = alias
        else:
            raise ValueError("unexpected APEX library node")


def prepare(archive, runtime, helper, output):
    base = verify_art_runtime(runtime)
    build = json.loads((helper / "build.json").read_text())
    if build.get("result") != "BUILT" or sha256(helper / "property-bootstrap") != build["binary_sha256"]:
        raise ValueError("property helper build is incomplete or changed")
    if elf_needed(helper / "property-bootstrap"):
        raise ValueError("property helper must be static ARM64")
    output.mkdir(parents=True, exist_ok=False)
    metadata = {"result": "PREPARING", "execution": "not tested", "sdk": base["sdk"],
                "base_manifest_sha256": sha256(runtime / "art-runtime.json"), "property_helper": build}
    manifest = output / "stock-inputs.json"
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    with tempfile.TemporaryDirectory(prefix="stock-inputs-", dir=output) as directory:
        temporary = Path(directory)
        metadata["partitions"] = extract_partitions(archive, temporary)
        images = {name: Ext4(temporary / f"{name}.ext4") for name in metadata["partitions"]}
        system = images["system"]
        records = []
        links = dict(BIONIC_ALIASES)
        (output / "apex").mkdir()
        for filename, mode, size in system.entries("/system/apex"):
            if not filename.endswith((".apex", ".capex")):
                continue
            if mode & 0o170000 != 0o100000:
                raise ValueError("factory APEX must be a regular file")
            package, payload = temporary / filename, temporary / (filename + ".ext4")
            source = "/system/apex/" + filename
            system.dump(source, package, size)
            original_hash = unpack_apex(package, payload)
            apex = Ext4(payload)
            original_manifest = temporary / "apex_manifest.pb"
            apex.dump("/apex_manifest.pb", original_manifest)
            name, version, version_name = apex_identity(original_manifest.read_bytes())
            destination = output / "apex" / name
            destination.mkdir()  # Duplicate module names are not silently merged.
            shutil.copyfile(original_manifest, destination / "apex_manifest.pb")
            entries = {entry[0] for entry in apex.entries("/")}
            if "etc" in entries:
                for entry, entry_mode, entry_size in apex.entries("/etc"):
                    if entry == "linker.config.pb" or (name == "com.android.vndk.v33" and entry.endswith(".txt")):
                        if entry_mode & 0o170000 != 0o100000:
                            raise ValueError("unexpected APEX configuration node")
                        apex.dump("/etc/" + entry, destination / "etc" / entry, entry_size)
            # Linkerconfig inspects executable/library directory presence.
            for lib in ("bin", "lib", "lib64"):
                if lib in entries:
                    (destination / lib).mkdir()
            if name == "com.android.runtime":
                apex.dump("/bin/linkerconfig", destination / "bin/linkerconfig")
            if name in base["apexes"] and "lib64" in entries:
                if base["apexes"][name]["apex_sha256"] != original_hash:
                    raise ValueError("base runtime uses a different APEX")
                stock_libraries(apex, "/lib64", f"apex/{name}/lib64", output, runtime, links)
            records.append({"name": name, "version": version, "version_name": version_name,
                            "source": source, "package_sha256": sha256(package),
                            "apex_sha256": original_hash, "payload_sha256": sha256(payload)})
            package.unlink()
            payload.unlink()
            original_manifest.unlink()
            embedded = payload.with_suffix(".apex")
            if embedded.exists():
                embedded.unlink()
        (output / "apex/apex-info-list.xml").write_bytes(activation_xml(records))
        metadata["factory_apexes"] = records
        props, contexts = [], []
        # Android 13 init/property_service.cpp PropertyLoadBootDefaults order.
        # This image has no second_stage_resources properties, vendor/default.prop,
        # system_ext/product contexts, or separate dlkm/odm property files.
        files = [
            ("system", "/system/build.prop", "system/build.prop"),
            ("system_ext", "/etc/build.prop", "system_ext/etc/build.prop"),
            ("vendor", "/build.prop", "vendor/build.prop"),
            ("product", "/etc/build.prop", "product/etc/build.prop"),
            ("system", "/system/etc/selinux/plat_property_contexts", "system/etc/selinux/plat_property_contexts"),
            ("vendor", "/etc/selinux/vendor_property_contexts", "vendor/etc/selinux/vendor_property_contexts"),
            ("system", "/system/etc/linker.config.pb", "system/etc/linker.config.pb"),
        ]
        for partition, source, target in files:
            images[partition].dump(source, output / target)
            if target.endswith(".prop"):
                props.append(output / target)
            elif target.endswith("_contexts"):
                contexts.append("/" + target)
        for name, mode, size in system.entries("/system/etc"):
            if ".libraries" in name and name.endswith(".txt"):
                system.dump("/system/etc/" + name, output / "system/etc" / name, size)

        # The original minimal runtime resolved dependencies across a flat search
        # path. Stock namespaces need the system partition's OWN implementations.
        # Never substitute an APEX's private libc++/libbase in the system namespace.
        entries = {name: (mode, size) for name, mode, size in system.entries("/system/lib64")}
        for alias, target in BIONIC_ALIASES.items():
            expected = "/apex/com.android.runtime/lib64/bionic/" + Path(alias).name
            stat = system.command('stat "/' + alias + '"')
            match = re.search(r'^Fast link dest: "([^"]+)"$', stat, re.MULTILINE)
            if not match or match[1] != expected:
                raise ValueError("unexpected system Bionic alias")
            path = output / alias
            path.parent.mkdir(parents=True, exist_ok=True)
            path.symlink_to(target)  # Same guest destination, bounded to the root on the host.
        queue = list((runtime / "system/lib64").glob("*.so"))
        queue += [p for p in (output / "apex").rglob("*.so") if p.is_file() and not p.is_symlink()]
        for alias, target in links.items():
            resolved = posixpath.normpath(posixpath.join(posixpath.dirname(alias), target))
            if resolved.startswith("system/lib64/") and not (runtime / resolved).is_file():
                path = output / resolved
                if not path.exists():
                    mode, size = entries[Path(resolved).name]
                    if mode & 0o170000 != 0o100000:
                        raise ValueError("unexpected APEX library alias target")
                    system.dump("/" + resolved, path, size)
                queue.append(path)
        seen = set()
        while queue:
            binary = queue.pop()
            if binary in seen:
                continue
            seen.add(binary)
            for name in elf_needed(binary):
                if name not in entries or "system/lib64/" + name in BIONIC_ALIASES:
                    continue
                target = output / "system/lib64" / name
                original = runtime / "system/lib64" / name
                if original.is_file():
                    queue.append(original)
                else:
                    if not target.exists():
                        mode, size = entries[name]
                        if mode & 0o170000 != 0o100000:
                            raise ValueError("unexpected system dependency node")
                        system.dump("/system/lib64/" + name, target, size)
                    queue.append(target)
        bootstrap = output / "data/local/tmp"
        bootstrap.mkdir(parents=True)
        literal = dict(line.split("=", 1) for line in merge_properties(props).splitlines())
        initialized = derive_cpu_abi_properties(literal)
        metadata["derived_boot_properties"] = {
            "algorithm": "android-13-init-cpu-abilist",
            "source": "https://android.googlesource.com/platform/system/core/+/83366e7e7a1556ab6cff117673bb9f17b973de99/init/property_service.cpp",
            "values": {key: value for key, value in initialized.items() if key not in literal},
        }
        (bootstrap / "property-values.prop").write_text(
            "".join(f"{key}={value}\n" for key, value in sorted(initialized.items())))
        (bootstrap / "property-contexts.list").write_text("\n".join(contexts) + "\n")
        shutil.copy2(helper / "property-bootstrap", bootstrap / "property-bootstrap")
    (output / "dev/__properties__").mkdir(parents=True)
    (output / "linkerconfig").mkdir()
    metadata.update({"result": "PREPARED", "symlinks": links,
                     "files": {str(path.relative_to(output)): sha256(path) for path in sorted(output.rglob("*"))
                               if path.is_file() and not path.is_symlink() and path != manifest},
                     "directories": [str(path.relative_to(output)) for path in sorted(output.rglob("*"))
                                     if path.is_dir() and not path.is_symlink()]})
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-image", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--property-helper", type=Path, required=True, help="build-properties.py output directory")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    prepare(args.sdk_image.resolve(), args.runtime_root.resolve(), args.property_helper.resolve(), args.output.resolve())
    print(args.output.resolve())


if __name__ == "__main__":
    main()
