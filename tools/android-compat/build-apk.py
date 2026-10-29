#!/usr/bin/env python3
"""Build the dependency-free Android UI/input probe using installed SDK tools."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import struct
import zipfile

from art_inputs import sha256, validate_dex

SOURCE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--build-tools", default="37.0.0")
    parser.add_argument("--platform", default="android-32")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime-probe", action="store_true", help="also build the stock-framework APK parser/resource/DEX probe")
    args = parser.parse_args()
    sdk, output = args.sdk.resolve(), args.output.resolve()
    jar = sdk / "platforms" / args.platform / "android.jar"
    build_tools = sdk / "build-tools" / args.build_tools
    for path in [jar, *(build_tools / n for n in ["aapt2", "d8", "zipalign", "apksigner"])]:
        if not path.is_file():
            parser.error(f"missing SDK input: {path}")
    output.mkdir(parents=True, exist_ok=False)
    classes, dex = output / "classes", output / "dex"
    classes.mkdir()
    dex.mkdir()
    java_bin = Path(os.environ["JAVA_HOME"]) / "bin" if "JAVA_HOME" in os.environ else None
    def java(name): return str(java_bin / name) if java_bin else name
    commands = []
    def run(command):
        commands.append(command)
        subprocess.run(command, check=True)
    initial_commands = [
        [java("javac"), "--release", "8", "-classpath", str(jar), "-d", str(classes), *(str(p) for p in sorted((SOURCE / "apk").glob("*.java")))],
        [str(build_tools / "aapt2"), "compile", "--dir", str(SOURCE / "apk/res"), "-o", str(output / "resources.zip")],
        [str(build_tools / "aapt2"), "link", "-I", str(jar), "--manifest", str(SOURCE / "apk/AndroidManifest.xml"), "-o", str(output / "unsigned.apk"), "-A", str(SOURCE / "apk/assets"), str(output / "resources.zip")],
    ]
    for command in initial_commands:
        run(command)
    run([str(build_tools / "d8"), "--lib", str(jar), "--min-api", "29", "--output", str(dex),
                    *(str(p) for p in sorted(classes.rglob("*.class")))])
    with zipfile.ZipFile(output / "unsigned.apk", "a", zipfile.ZIP_DEFLATED) as archive:
        archive.write(dex / "classes.dex", "classes.dex")
    run([str(build_tools / "zipalign"), "4", str(output / "unsigned.apk"), str(output / "aligned.apk")])
    # This disposable key signs only the public test fixture, never a user APK.
    run([java("keytool"), "-genkeypair", "-keystore", str(output / "test.keystore"), "-storepass", "android",
                    "-keypass", "android", "-alias", "scarlet-test", "-dname", "CN=Scarlet compatibility test",
                    "-keyalg", "RSA", "-validity", "3650"])
    apk = output / "scarlet-compat.apk"
    run([str(build_tools / "apksigner"), "sign", "--ks", str(output / "test.keystore"),
                    "--ks-pass", "pass:android", "--out", str(apk), str(output / "aligned.apk")])
    run([str(build_tools / "apksigner"), "verify", str(apk)])
    metadata = {"platform": args.platform, "build_tools": args.build_tools,
        "commands": commands,
        "tools_sha256": {name: sha256(build_tools / name) for name in ("aapt2", "zipalign", "lib/d8.jar", "lib/apksigner.jar")},
        "package": "dev.scarlet.compat", "sha256": sha256(apk), "framework_sdk_sha256": sha256(jar),
        "sources": {str(p.relative_to(SOURCE)): sha256(p) for p in sorted((SOURCE / "apk").rglob("*")) if p.is_file()}}
    if args.runtime_probe:
        loader = output / "loader"
        loader.mkdir()
        source = SOURCE / "framework/ScarletApkProbe.java"
        run([java("javac"), "--release", "8", "-classpath", str(jar), "-d", str(loader), str(source)])
        run([str(build_tools / "d8"), "--lib", str(jar), "--min-api", "23", "--output", str(output),
                        *(str(p) for p in sorted(loader.rglob("*.class")))])
        # Corrupt one signed asset byte while preserving ZIP metadata and the
        # APK signing block, so the guest must detect a content-digest failure.
        tampered = bytearray(apk.read_bytes())
        with zipfile.ZipFile(apk) as archive:
            entry = archive.getinfo("assets/probe.txt")
            if entry.compress_size == 0:
                raise ValueError("empty fixture asset")
            name_size, extra_size = struct.unpack_from("<HH", tampered, entry.header_offset + 26)
            offset = entry.header_offset + 30 + name_size + extra_size
            tampered[offset] ^= 1
        (output / "scarlet-tampered.apk").write_bytes(tampered)
        metadata["tampered_sha256"] = sha256(output / "scarlet-tampered.apk")
        metadata.update(main_class="ScarletApkProbe", source_sha256=sha256(source), dex_sha256=validate_dex(output / "classes.dex"))
    (output / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(apk)


if __name__ == "__main__":
    main()
