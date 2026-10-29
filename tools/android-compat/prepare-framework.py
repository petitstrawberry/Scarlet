#!/usr/bin/env python3
"""Extract stock app_process64, boot classpath fragments/JARs and startup JNI.

This is a fixture overlay bound to one pinned SDK image and verified base/stock
inputs. It does not boot Android init, zygote, system_server or an application.
"""
import argparse
import json
from pathlib import Path
import tempfile

from art_inputs import Ext4, extract_system, sha256, unpack_apex, verify_art_runtime, elf_needed
from framework_inputs import PROGRAM, boot_classpath, verify_framework_inputs
from stock_art import verify_stock_inputs, apex_identity


def prepare(archive, runtime, stock, output):
    verify_art_runtime(runtime)
    stock_metadata = verify_stock_inputs(stock, runtime)
    output.mkdir(parents=True, exist_ok=False)
    metadata = {"result": "PREPARING", "kind": "stock-framework-inputs-v1", "execution": "not tested",
                "base_manifest_sha256": sha256(runtime / "art-runtime.json"),
                "stock_manifest_sha256": sha256(stock / "stock-inputs.json")}
    manifest = output / "framework-inputs.json"
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    with tempfile.TemporaryDirectory(prefix="framework-", dir=output) as directory:
        temporary = Path(directory)
        image = temporary / "system.ext4"
        metadata["sdk"] = extract_system(archive, image)
        system = Ext4(image)
        system.dump("/" + PROGRAM, output / PROGRAM)
        system.dump("/system/framework/framework-res.apk", output / "system/framework/framework-res.apk")
        system.dump("/system/etc/classpaths/bootclasspath.pb", output / "system/etc/classpaths/bootclasspath.pb")
        apexes = []
        known = {record["name"]: record for record in stock_metadata["factory_apexes"]}
        for filename, mode, size in system.entries("/system/apex"):
            if not filename.endswith((".apex", ".capex")):
                continue
            if mode & 0o170000 != 0o100000:
                raise ValueError("APEX package is not a regular image file")
            package, payload = temporary / filename, temporary / (filename + ".ext4")
            system.dump("/system/apex/" + filename, package, size)
            identity = unpack_apex(package, payload)
            apex = Ext4(payload)
            identity_file = temporary / "apex_manifest.pb"
            apex.dump("/apex_manifest.pb", identity_file)
            module, _, _ = apex_identity(identity_file.read_bytes())
            if identity != known[module]["apex_sha256"]:
                raise ValueError("framework APEX differs from stock inputs")
            target = output / "apex" / module
            entries = {name for name, _, _ in apex.entries("/")}
            if "etc" in entries and "classpaths" in {name for name, _, _ in apex.entries("/etc")}:
                if "bootclasspath.pb" in {name for name, _, _ in apex.entries("/etc/classpaths")}:
                    apex.dump("/etc/classpaths/bootclasspath.pb", target / "etc/classpaths/bootclasspath.pb")
            # Retain just source JARs for now; selected boot JARs are filtered below.
            if "javalib" in entries:
                for name, node_mode, node_size in apex.entries("/javalib"):
                    if name.endswith(".jar"):
                        if node_mode & 0o170000 != 0o100000:
                            raise ValueError("boot JAR must be a regular image file")
                        apex.dump("/javalib/" + name, target / "javalib" / name, node_size)
            # RuntimeInit loads TrafficStats JNI; APK resources/signatures load
            # the image's Conscrypt provider through the same stock linker config.
            if module in {"com.android.tethering", "com.android.conscrypt"}:
                for name, node_mode, node_size in apex.entries("/lib64"):
                    if name.endswith(".so"):
                        if node_mode & 0o170000 != 0o100000:
                            raise ValueError("unexpected tethering library node")
                        apex.dump("/lib64/" + name, target / "lib64" / name, node_size)
            apexes.append({"name": module, "apex_sha256": identity, "payload_sha256": sha256(payload)})
            package.unlink()
            payload.unlink()
            identity_file.unlink()
            embedded = payload.with_suffix(".apex")
            if embedded.exists():
                embedded.unlink()
        metadata["apexes"] = apexes
        classpath, fragments = boot_classpath(output)
        metadata.update(boot_class_path=classpath, fragments=fragments)
        for name in classpath:
            if name.startswith("/system/framework/"):
                system.dump(name, output / name.lstrip("/"))
            if not (output / name.lstrip("/")).is_file():
                raise ValueError("missing image boot JAR: " + name)
        for jar in output.glob("apex/*/javalib/*.jar"):
            if "/" + str(jar.relative_to(output)) not in classpath:
                jar.unlink()
    libraries = [output / PROGRAM] + sorted(output.glob("apex/*/lib64/*.so"))
    metadata["needed"] = {str(p.relative_to(output)): elf_needed(p) for p in libraries}
    metadata.update(result="PREPARED", files={str(p.relative_to(output)): sha256(p) for p in sorted(output.rglob("*"))
                                              if p.is_file() and p != manifest})
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    verify_framework_inputs(output, runtime, stock)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-image", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--stock-inputs", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    prepare(args.sdk_image.resolve(), args.runtime_root.resolve(), args.stock_inputs.resolve(), args.output.resolve())
    print(args.output.resolve())


if __name__ == "__main__":
    main()
