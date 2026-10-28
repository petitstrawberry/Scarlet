#!/usr/bin/env python3
"""Package native Scarlet java and built java.base classes using a matching host JDK.

Normally invoked by build.py --target java. Standalone usage:
  python3 tools/openjdk/image.py --source /prepared/jdk --build /configured/build \
      --boot-jdk /host/jdk-25 --java /native/java --output /new/image

The output contains bin/java, usr/lib/jvm/openjdk-<major>, app/, and image.json.
The caller must ELF-audit the native launcher. This helper never runs it.
The installed boot JDK and target class directory are read-only inputs. A
temporary OperatingSystem enum patch lets host jlink recognize SCARLET while
retaining the host's PlatformProps; that patch is never packaged in the image.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import build as bootstrap


OWNER = {"owner": "scarlet-openjdk-image", "format": 1}
TARGET_PLATFORM = "scarlet-aarch64"
OS_SOURCE = Path("src/java.base/share/classes/jdk/internal/util/OperatingSystem.java")
OS_CLASS = Path("jdk/internal/util/OperatingSystem.class")
PAYLOAD_DIRS = ("usr", "bin", "app")


def host_os_source(data):
    anchor = b"return PlatformProps.TARGET_OS_IS_SCARLET;"
    if data.count(anchor) != 1:
        raise bootstrap.BuildError("Expected one Scarlet isScarlet() anchor in OperatingSystem.java")
    return data.replace(anchor, b"return false;")


def payload_identity(root):
    trees = {}
    for name in PAYLOAD_DIRS:
        path = root / name
        if path.is_symlink() or not path.is_dir():
            raise bootstrap.BuildError(f"Missing regular image directory: {path}")
        trees[name] = bootstrap.tree_identity(path)
    return bootstrap.identity(trees)


def package_image(source, jdk_build, boot_jdk, java, output, env):
    source, jdk_build = source.resolve(), jdk_build.resolve()
    # Check before resolving the executable itself, so symlink launchers cannot
    # disguise an input/output overlap or a mutable external artifact.
    java = bootstrap.regular_file(java.expanduser().absolute()).resolve()
    major, module_version = bootstrap.source_version(source)
    guest_home = Path(f"usr/lib/jvm/openjdk-{major}")
    boot_jdk, boot_identity = bootstrap.validate_boot_jdk(boot_jdk, env, major)
    output = output.expanduser().absolute()
    canonical_output = output.resolve()
    for label, path in (("boot JDK", boot_jdk), ("source", source), ("build", jdk_build), ("java", java)):
        path = path.resolve()
        if (canonical_output == path or canonical_output in path.parents or path in canonical_output.parents):
            raise bootstrap.BuildError(f"Image output overlaps {label} input: {output}")
    classes = jdk_build / "jdk/modules/java.base"
    bootstrap.regular_file(classes / "module-info.class")
    original_os = bootstrap.regular_file(source / OS_SOURCE).read_bytes()
    host_os = host_os_source(original_os)
    legal = source / "src/java.base/share/legal"
    notices = ("LICENSE", "ADDITIONAL_LICENSE_INFO", "ASSEMBLY_EXCEPTION")
    inputs = {"source": str(source), "build": str(jdk_build), "boot_jdk": boot_identity,
              "java": str(java), "java_sha256": bootstrap.sha256(java),
              "classes_sha256": bootstrap.tree_identity(classes),
              "operating_system_sha256": hashlib.sha256(original_os).hexdigest(),
              "host_patch_source_sha256": hashlib.sha256(host_os).hexdigest(),
              "image_helper_sha256": bootstrap.sha256(Path(__file__)),
              "build_helper_sha256": bootstrap.sha256(Path(__file__).with_name("build.py")),
              "legal_sha256": bootstrap.tree_identity(legal),
              "notices": {name: bootstrap.sha256(bootstrap.regular_file(source / name)) for name in notices},
              "module_version": module_version, "target_platform": TARGET_PLATFORM}
    with bootstrap.output_lock(output, OWNER):
        output = output.resolve()
        receipt_path = output / "image.json"
        if bootstrap.exists(receipt_path):
            receipt = json.loads(bootstrap.regular_file(receipt_path).read_text())
            if receipt["inputs"] != inputs:
                raise bootstrap.BuildError(f"Image inputs changed; preserve {output} and choose a new --output")
            if receipt["payload_sha256"] != payload_identity(output):
                raise bootstrap.BuildError(f"Packaged image was modified: {output}; refusing to overwrite it")
            bootstrap.log(f"Reusing verified image: {output}")
            return receipt
        if any(bootstrap.exists(output / name) for name in PAYLOAD_DIRS):
            raise bootstrap.BuildError(f"Image output has unrecorded payload files: {output}; choose a new --output")
        logs = bootstrap.directory(output / "logs")
        with tempfile.TemporaryDirectory(prefix=".packaging-", dir=output) as temporary:
            stage = Path(temporary)
            image = stage / "image"
            image.mkdir()
            jmods = bootstrap.directory(stage / "jmods")
            bootstrap.run([boot_jdk / "bin/jmod", "create", "--class-path", classes,
                           "--legal-notices", legal,
                           "--module-version", module_version, "--target-platform", TARGET_PLATFORM,
                           jmods / "java.base.jmod"], stage, logs / "jmod.log", env)
            bootstrap.regular_file(jmods / "java.base.jmod")

            host_source = stage / "host-source"
            enum_source = host_source / "jdk/internal/util/OperatingSystem.java"
            enum_source.parent.mkdir(parents=True)
            enum_source.write_bytes(host_os)
            host_patch = bootstrap.directory(stage / "host-patch")
            # Only this one source is visible. PlatformProps and all other host
            # classes must come from the installed boot JDK's java.base module.
            bootstrap.run([boot_jdk / "bin/javac", "-implicit:none", "--patch-module",
                           f"java.base={host_source}", "-d", host_patch, enum_source],
                          stage, logs / "host-patch.log", env)
            generated = {path.relative_to(host_patch) for path in host_patch.rglob("*") if not path.is_dir()}
            if generated != {OS_CLASS}:
                raise bootstrap.BuildError("Host patch must contain only OperatingSystem.class")
            bootstrap.regular_file(host_patch / OS_CLASS)

            java_home = image / guest_home
            java_home.parent.mkdir(parents=True)
            bootstrap.run([boot_jdk / "bin/jlink", f"-J--patch-module=java.base={host_patch}",
                           "--module-path", jmods, "--add-modules", "java.base", "--output", java_home,
                           "--no-header-files", "--no-man-pages"], stage, logs / "jlink.log", env)
            modules = bootstrap.regular_file(java_home / "lib/modules")
            bootstrap.regular_file(java_home / "release")
            if not modules.stat().st_size:
                raise bootstrap.BuildError("jlink produced an empty lib/modules")
            # Install only the native executable, never a host launcher or the
            # temporary jlink patch. /app is available for explicitly selected classes.
            for name in notices:
                shutil.copyfile(source / name, java_home / name)
            (image / "bin").mkdir()
            shutil.copyfile(java, image / "bin/java")
            (image / "bin/java").chmod(0o755)
            (image / "app").mkdir()
            if (bootstrap.sha256(image / "bin/java") != inputs["java_sha256"]
                    or bootstrap.tree_identity(classes) != inputs["classes_sha256"]
                    or (source / OS_SOURCE).read_bytes() != original_os):
                raise bootstrap.BuildError("Packaging inputs changed during image creation; image not published")
            receipt = {"inputs": inputs, "artifact": str(output / "bin/java"),
                       "java_sha256": inputs["java_sha256"], "modules_sha256": bootstrap.sha256(modules),
                       "image_root": str(output), "java_home": str(output / guest_home),
                       "guest_java_home": "/" + guest_home.as_posix(), "guest_class_path": ".",
                       "payload_sha256": payload_identity(image),
                       "scope": "Native Java and Scarlet java.base packaged; not guest-tested"}
            # Publish only completed payloads. Never replace an existing tree;
            # an interrupted publication is reported on retry, not reset.
            for name in PAYLOAD_DIRS:
                if bootstrap.exists(output / name):
                    raise bootstrap.BuildError(f"Refusing to overwrite image payload: {output / name}")
            for name in PAYLOAD_DIRS:
                (image / name).rename(output / name)
            bootstrap.write_json(receipt_path, receipt)
        bootstrap.log(f"Packaged native Java image: {output}\n  Receipt: {receipt_path}")
        return receipt


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for name in ("source", "build", "boot-jdk", "java", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args(argv)
    env = dict(os.environ, LC_ALL="C")
    try:
        package_image(args.source, args.build, args.boot_jdk, args.java, args.output, env)
    except (bootstrap.BuildError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"[openjdk-image] ERROR: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("[openjdk-image] Interrupted; inputs were not reset", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
