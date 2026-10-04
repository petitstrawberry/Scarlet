#!/usr/bin/env python3
"""Build the checked-out Limine BSP and modules without resolving image bundles."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("aarch64", "riscv64"), required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    template = repo / "projects" / f"{args.arch}-limine-full"
    source_bsp = template / "bsp"
    project = repo / "target" / "ci-kernel" / args.arch
    bsp = project / "bsp"
    bsp.mkdir(parents=True, exist_ok=True)
    for directory in (".cargo", "src", "lds"):
        destination = bsp / directory
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(source_bsp / directory, destination)
    for filename in ("Cargo.toml", "build.rs"):
        shutil.copyfile(source_bsp / filename, bsp / filename)

    config_path = bsp / ".cargo" / "config.toml"
    config_text = config_path.read_text()
    target = tomllib.loads(config_text)["build"]["target"]
    config_path.write_text(config_text.replace(
        json.dumps(target), json.dumps(str((source_bsp / target).resolve())), 1
    ))

    # Keep the template's BSP features and enabled modules. Image layers are
    # exercised by image builds, not by this kernel build job.
    manifest = (template / "scarlet.toml").read_text().split("[userspace]", 1)[0]
    original = tomllib.loads(manifest)
    lines = manifest.splitlines()
    section = ""
    for index, line in enumerate(lines):
        if line.startswith("["):
            section = line
        if section == "[bsp.kernel]" and line.startswith("source ="):
            lines[index] = "source = { path = " + json.dumps(str(repo / "kernel")) + " }"
        elif section == "[modules]" and line.startswith('"scarlet-module-prototype" ='):
            enabled = original["modules"]["scarlet-module-prototype"]["enabled"]
            lines[index] = '"scarlet-module-prototype" = { path = ' + json.dumps(
                str(repo / "modules" / "scarlet-module-prototype")
            ) + ", enabled = " + str(enabled).lower() + " }"
    generated = "\n".join(lines) + "\n"
    actual = tomllib.loads(generated)
    expected = original
    expected["bsp"]["kernel"]["source"] = {"path": str(repo / "kernel")}
    expected["modules"]["scarlet-module-prototype"].pop("git", None)
    expected["modules"]["scarlet-module-prototype"].pop("rev", None)
    expected["modules"]["scarlet-module-prototype"]["path"] = str(
        repo / "modules" / "scarlet-module-prototype"
    )
    expected_target = str((source_bsp / target).resolve())
    if actual != expected or tomllib.loads(config_path.read_text())["build"]["target"] != expected_target:
        raise RuntimeError("CI project must preserve BSP configuration and use local sources")
    (project / "scarlet.toml").write_text(generated)
    subprocess.run(["cargo", "scarlet", "build", "--project", str(project)], cwd=repo, check=True)


if __name__ == "__main__":
    main()
