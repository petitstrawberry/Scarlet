#!/usr/bin/env python3
"""Build the dependency-free Android UI/input probe using installed SDK tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import zipfile

SOURCE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--build-tools", default="37.0.0")
    parser.add_argument("--platform", default="android-32")
    parser.add_argument("--output", type=Path, required=True)
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
    commands = [
        [java("javac"), "--release", "8", "-classpath", str(jar), "-d", str(classes), str(SOURCE / "apk/MainActivity.java")],
        [str(build_tools / "aapt2"), "link", "-I", str(jar), "--manifest", str(SOURCE / "apk/AndroidManifest.xml"), "-o", str(output / "unsigned.apk")],
    ]
    for command in commands:
        subprocess.run(command, check=True)
    subprocess.run([str(build_tools / "d8"), "--lib", str(jar), "--min-api", "29", "--output", str(dex),
                    *(str(p) for p in sorted(classes.rglob("*.class")))], check=True)
    with zipfile.ZipFile(output / "unsigned.apk", "a", zipfile.ZIP_DEFLATED) as archive:
        archive.write(dex / "classes.dex", "classes.dex")
    subprocess.run([str(build_tools / "zipalign"), "4", str(output / "unsigned.apk"), str(output / "aligned.apk")], check=True)
    # This disposable key signs only the public test fixture, never a user APK.
    subprocess.run([java("keytool"), "-genkeypair", "-keystore", str(output / "test.keystore"), "-storepass", "android",
                    "-keypass", "android", "-alias", "scarlet-test", "-dname", "CN=Scarlet compatibility test",
                    "-keyalg", "RSA", "-validity", "3650"], check=True)
    apk = output / "scarlet-compat.apk"
    subprocess.run([str(build_tools / "apksigner"), "sign", "--ks", str(output / "test.keystore"),
                    "--ks-pass", "pass:android", "--out", str(apk), str(output / "aligned.apk")], check=True)
    subprocess.run([str(build_tools / "apksigner"), "verify", str(apk)], check=True)
    (output / "build.json").write_text(json.dumps({"platform": args.platform, "build_tools": args.build_tools,
        "package": "dev.scarlet.compat", "sha256": hashlib.sha256(apk.read_bytes()).hexdigest()}, indent=2) + "\n")
    print(apk)


if __name__ == "__main__":
    main()
