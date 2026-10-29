#!/usr/bin/env python3
"""Build the control-socket probe for a verified stock Android framework."""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess

from art_inputs import sha256, validate_dex
from framework_inputs import verify_framework_inputs
from zygote_inputs import SOURCE, header


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("ndk", "framework-inputs", "runtime-root", "stock-inputs", "output"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--child", action="store_true", help="verify a specialized child entering the fixture Java main")
    p.add_argument("--build-tools", type=Path)
    p.add_argument("--framework-sdk", type=Path)
    args = p.parse_args()
    if args.child and (not args.build_tools or not args.framework_sdk):
        p.error("--child requires --build-tools and --framework-sdk")
    metadata = verify_framework_inputs(args.framework_inputs, args.runtime_root, args.stock_inputs)
    host = {"Darwin": "darwin-x86_64", "Linux": "linux-x86_64"}[platform.system()]
    compiler = args.ndk / "toolchains/llvm/prebuilt" / host / "bin/aarch64-linux-android33-clang"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "zygote-config.h").write_text(header(metadata, args.child))
    command = [str(compiler), "-O2", "-Wall", "-Wextra", "-Werror", "-I", str(output),
               str(SOURCE / "zygote-probe.c"), "-o", str(output / "zygote-probe")]
    subprocess.run(command, check=True)
    record = {"kind": "stock-zygote-child-v1" if args.child else "stock-zygote-control-v2", "command": command,
              "source_sha256": sha256(SOURCE / "zygote-probe.c"),
              "framework_sha256": sha256(args.framework_inputs / "framework-inputs.json"),
              "program_sha256": sha256(output / "zygote-probe")}
    if args.child:
        java = str(Path(os.environ["JAVA_HOME"]) / "bin/java") if "JAVA_HOME" in os.environ else "java"
        d8 = args.build_tools.resolve() / "lib/d8.jar"
        source = SOURCE / "framework/ScarletZygoteChild.java"
        classes = output / "classes"; classes.mkdir()
        compile_command = [java, "com.sun.tools.javac.Main", "--release", "8", "-classpath",
            str(args.framework_sdk.resolve()), "-d", str(classes), str(source)]
        dex_command = [java, "-cp", str(d8), "com.android.tools.r8.D8", "--min-api", "23",
            "--lib", str(args.framework_sdk.resolve()), "--output", str(output)]
        subprocess.run(compile_command, check=True)
        dex_command += [str(p) for p in sorted(classes.rglob("*.class"))]
        subprocess.run(dex_command, check=True)
        record.update(child_source_sha256=sha256(source), child_dex_sha256=validate_dex(output / "classes.dex"),
            framework_sdk_sha256=sha256(args.framework_sdk), d8_sha256=sha256(d8), child_commands=[compile_command,dex_command])
    (output / "build.json").write_text(json.dumps(record, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
