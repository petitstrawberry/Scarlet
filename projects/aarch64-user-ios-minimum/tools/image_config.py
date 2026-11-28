"""Resolve packaging inputs from the selected SDK project manifest."""

from pathlib import Path
import os
import sys
import tomllib


def image_path(project, field, target=None, profile=None):
    project = Path(project).resolve()
    if field in ("target-dir", "executable"):
        override = os.environ.get("CARGO_TARGET_DIR")
        # Resolve relative overrides before the SDK changes into the BSP or
        # project directory. The wrapper exports this absolute path to hooks.
        directory = Path(override).resolve() if override else image_path(project, "bsp") / "target"
        if field == "target-dir":
            return directory
        if not target or not profile:
            raise ValueError("Executable lookup requires a target and profile")
        return directory / target / profile / "scarlet"
    with (project / "scarlet.toml").open("rb") as source:
        manifest = tomllib.load(source)
    if field == "bsp":
        relative = manifest["bsp"]["path"]
    else:
        relative = manifest["images"][field]["output"]
    return (project / relative).resolve()


if __name__ == "__main__":
    print(image_path(*sys.argv[1:]))
