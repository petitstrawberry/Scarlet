"""Verify the staged Desktop distribution and its signed Simulator package."""

import hashlib
import plistlib
from pathlib import Path
import subprocess
import sys
import tomllib


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify(project, app):
    with (app / "Info.plist").open("rb") as source:
        info = plistlib.load(source)
        if info.get("ScarletDefaultDisplay") is not True:
            raise ValueError("Desktop must set boolean ScarletDefaultDisplay=true")
        if info.get("CFBundleDisplayName") != "Scarlet Desktop":
            raise ValueError("Desktop must have the Home screen label Scarlet Desktop")
    shared = project.parent / "aarch64-user-ios-minimum"
    staging = project / ".scarlet/staging"
    root = staging / "rootfs"
    for name in ("sws", "scarlet-seat", "scarlet-desktop", "scarlet-shell", "terminal"):
        if not (root / "bin" / name).is_file():
            raise ValueError(f"Missing stock Desktop entrypoint: {name}")
    driver = root / "lib/sgfx/libsgfx_hosted.so"
    descriptor = root / "lib/sgfx/hosted-wgpu.sgfx-driver"
    original = project.parent / "aarch64-user-macos-full/bsp/sgfx-driver/hosted-wgpu.sgfx-driver"
    if descriptor.read_bytes() != original.read_bytes():
        raise ValueError("Hosted SGFX descriptor differs from its source")
    catalog = subprocess.check_output([
        sys.executable, shared / "tools/catalog_objects.py",
        "--exclusions", project / "ios-catalog-exclusions.json",
        staging / "initramfs", root,
    ]).split(b"\0")[:-1]
    registered = set((app / "guest-images").read_text().splitlines())
    expected = {f"ScarletImage{value.decode()}" for value in catalog[1::3]}
    if registered != expected | {"ScarletGuest"}:
        raise ValueError("Packaged catalog does not match the staged ELF objects")
    hosted_name = f"ScarletImage{digest(driver)}"
    if hosted_name not in expected:
        raise ValueError("Hosted SGFX DSO is absent from the trusted catalog")
    for name in registered:
        if not (app / "Frameworks" / f"{name}.framework" / name).is_file():
            raise ValueError(f"Missing framework: {name}")
    with (project / "scarlet.toml").open("rb") as source:
        manifest = tomllib.load(source)
    for image, bundled in (("initramfs", "initramfs.cpio"), ("rootfs", "root.ext2")):
        if digest(project / manifest["images"][image]["output"]) != digest(app / bundled):
            raise ValueError(f"Packaged {image} differs from the SDK image")
    subprocess.run(["codesign", "--verify", "--deep", "--strict", app], check=True)
    print(f"PASS: stock Desktop, hosted SGFX, {len(expected)} catalog objects, images and signatures")
    print(f"Hosted SGFX framework: {hosted_name}")


if __name__ == "__main__":
    project = Path(__file__).resolve().parents[1]
    app = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(
        (project / ".scarlet/images/iphonesimulator-release.app-path").read_text().strip()
    )
    verify(project, app)
