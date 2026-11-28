"""Adapt custom BSP output paths to the pinned cargo-scarlet CLI."""

import os
from pathlib import Path
import subprocess
import sys
import tomllib

from image_config import image_path


def commands(project, operation, args):
    project = Path(project).resolve()
    base = ["cargo", "scarlet"]
    original = base + [operation, "--project", str(project), *args]
    options = args[:args.index("--")] if "--" in args else args
    canonical = (image_path(project, "bsp") / "target").resolve()
    selected = image_path(project, "target-dir")
    if (operation not in ("image", "run") or selected == canonical
            or "--help" in options or "-h" in options
            or (operation == "run" and "--no-image" in options)):
        return [original]

    # image accepts --kernel-elf; run's internal image call does not forward one.
    # Preserve an explicit SDK input rather than adding a duplicate CLI option.
    if operation == "image" and any(
        arg == "--kernel-elf" or arg.startswith("--kernel-elf=") for arg in options
    ):
        return [original]
    target = None
    for index, arg in enumerate(options):
        if arg.startswith("--target="):
            target = arg.split("=", 1)[1]
        elif arg == "--target":
            if index + 1 == len(options):
                return [original]  # Let the SDK report the malformed option.
            target = options[index + 1]
    if target is None:
        with (image_path(project, "bsp") / ".cargo/config.toml").open("rb") as source:
            target = tomllib.load(source)["build"]["target"]
    profile = "release" if "--release" in options else "debug"
    executable = str(image_path(project, "executable", target, profile))
    if operation == "image":
        return [original + ["--kernel-elf", executable]]
    image = base + ["image", "--project", str(project), *options, "--kernel-elf", executable]
    runtime = args[len(options):]
    run = base + ["run", "--project", str(project), *options, "--no-image", *runtime]
    return [image, run]


def main():
    project, operation, *args = sys.argv[1:]
    # Direct invocations follow the same caller-cwd rule as scarlet_ios.sh.
    os.environ["CARGO_TARGET_DIR"] = str(image_path(project, "target-dir"))
    planned = commands(project, operation, args)
    for command in planned[:-1]:
        result = subprocess.run(command)
        if result.returncode:
            raise SystemExit(result.returncode)
    os.execvp(planned[-1][0], planned[-1])


if __name__ == "__main__":
    main()
