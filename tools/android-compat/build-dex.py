#!/usr/bin/env python3
"""Build a small plain-Java DEX with a local JDK and Android SDK D8."""
import argparse
import json
import os
from pathlib import Path
import subprocess

from art_inputs import sha256, validate_dex

SOURCE = Path(__file__).resolve().parent / "art/ScarletArtHello.java"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-tools", type=Path, required=True, help="SDK Build Tools directory containing lib/d8.jar")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    d8 = args.build_tools.resolve() / "lib/d8.jar"
    if not d8.is_file():
        parser.error(f"missing D8: {d8}")
    java = str(Path(os.environ["JAVA_HOME"]) / "bin/java") if "JAVA_HOME" in os.environ else "java"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    classes = output / "classes"
    classes.mkdir()
    commands = [
        [java, "com.sun.tools.javac.Main", "--release", "8", "-d", str(classes), str(SOURCE)],
        [java, "-cp", str(d8), "com.android.tools.r8.D8", "--min-api", "23", "--output", str(output),
         str(classes / "ScarletArtHello.class")],
    ]
    for command in commands:
        subprocess.run(command, check=True)
    def version(command):
        return subprocess.run(command, check=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True).stdout.strip()
    metadata = {"main_class": "ScarletArtHello", "source_sha256": sha256(SOURCE),
        "dex_sha256": validate_dex(output / "classes.dex"), "d8_sha256": sha256(d8),
        "java_version": version([java, "-version"]),
        "javac_version": version([java, "com.sun.tools.javac.Main", "-version"]),
        "d8_version": version([java, "-cp", str(d8), "com.android.tools.r8.D8", "--version"]),
        "commands": commands, "execution": "not tested"}
    (output / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(output / "classes.dex")


if __name__ == "__main__":
    main()
