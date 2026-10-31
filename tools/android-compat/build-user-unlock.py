#!/usr/bin/env python3
"""Build the bounded Android user-0 unlock helper DEX."""
import argparse
import json
from pathlib import Path
import subprocess

from art_inputs import sha256, validate_dex

SOURCE = Path(__file__).resolve().parent / "framework/ScarletUserUnlock.java"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-tools", type=Path, required=True)
    parser.add_argument("--framework-sdk", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error("output must not exist")
    output.mkdir(parents=True)
    classes = output / "classes"
    classes.mkdir()
    d8 = args.build_tools.resolve() / "lib/d8.jar"
    command = ["javac", "--release", "8", "-classpath",
               str(args.framework_sdk.resolve()), "-d", str(classes), str(SOURCE)]
    subprocess.run(command, check=True)
    dex = [str(output / "classes.dex")]
    dex_command = ["java", "-cp", str(d8), "com.android.tools.r8.D8", "--min-api", "23",
                   "--lib", str(args.framework_sdk.resolve()), "--output", str(output)]
    dex_command.extend(str(path) for path in sorted(classes.rglob("*.class")))
    subprocess.run(dex_command, check=True)
    record = {
        "kind": "scarlet-user-unlock-v1",
        "result": "BUILT",
        "main_class": SOURCE.stem,
        "source_sha256": sha256(SOURCE),
        "framework_sdk_sha256": sha256(args.framework_sdk),
        "dex_sha256": validate_dex(output / "classes.dex"),
        "commands": [command, dex_command],
    }
    (output / "build.json").write_text(json.dumps(record, indent=2) + "\n")
    print(output / "classes.dex")


if __name__ == "__main__":
    main()
