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
    parser.add_argument("--framework-sdk", type=Path, help="SDK android.jar; build the framework/JNI probe")
    args = parser.parse_args()
    source = SOURCE.parent.parent / "framework/ScarletFrameworkHello.java" if args.framework_sdk else SOURCE
    if args.framework_sdk and not args.framework_sdk.is_file():
        parser.error("--framework-sdk must be an Android SDK android.jar")
    d8 = args.build_tools.resolve() / "lib/d8.jar"
    if not d8.is_file():
        parser.error(f"missing D8: {d8}")
    java = str(Path(os.environ["JAVA_HOME"]) / "bin/java") if "JAVA_HOME" in os.environ else "java"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    classes = output / "classes"
    classes.mkdir()
    compile_command = [java, "com.sun.tools.javac.Main", "--release", "8", "-d", str(classes)]
    if args.framework_sdk:
        compile_command += ["-classpath", str(args.framework_sdk.resolve())]
    compile_command.append(str(source))
    subprocess.run(compile_command, check=True)
    dex_command = [java, "-cp", str(d8), "com.android.tools.r8.D8", "--min-api", "23", "--output", str(output)]
    if args.framework_sdk:
        dex_command += ["--lib", str(args.framework_sdk.resolve())]
    dex_command += [str(path) for path in sorted(classes.rglob("*.class"))]
    subprocess.run(dex_command, check=True)
    commands = [compile_command, dex_command]
    def version(command):
        return subprocess.run(command, check=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True).stdout.strip()
    metadata = {"main_class": source.stem, "source_sha256": sha256(source),
        "framework_sdk_sha256": sha256(args.framework_sdk) if args.framework_sdk else None,
        "dex_sha256": validate_dex(output / "classes.dex"), "d8_sha256": sha256(d8),
        "java_version": version([java, "-version"]),
        "javac_version": version([java, "com.sun.tools.javac.Main", "-version"]),
        "d8_version": version([java, "-cp", str(d8), "com.android.tools.r8.D8", "--version"]),
        "commands": commands, "execution": "not tested"}
    (output / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(output / "classes.dex")


if __name__ == "__main__":
    main()
