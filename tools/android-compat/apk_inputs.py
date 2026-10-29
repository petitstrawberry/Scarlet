"""Bind the APK runtime fixture to its Java, resources, signed APK and loader DEX."""
import json
from pathlib import Path
import shutil
import zipfile

from art_inputs import sha256, validate_dex

SOURCE = Path(__file__).resolve().parent
APK = "data/local/tmp/scarlet-compat.apk"
FRAMEWORK_RESOURCES = "system/framework/framework-res.apk"


def verify_apk_probe(build, framework):
    record = json.loads((build / "build.json").read_text())
    sources = {str(p.relative_to(SOURCE)): sha256(p) for p in sorted((SOURCE / "apk").rglob("*")) if p.is_file()}
    if (record.get("main_class") != "ScarletApkProbe" or not record.get("framework_sdk_sha256")
            or record.get("package") != "dev.scarlet.compat" or record.get("sources") != sources
            or record.get("source_sha256") != sha256(SOURCE / "framework/ScarletApkProbe.java")
            or record.get("dex_sha256") != validate_dex(build / "classes.dex")
            or record.get("sha256") != sha256(build / "scarlet-compat.apk")
            or record.get("tampered_sha256") != sha256(build / "scarlet-tampered.apk")):
        raise ValueError("changed or mismatched APK runtime probe")
    with zipfile.ZipFile(build / "scarlet-compat.apk") as archive:
        names = archive.namelist()
        if len(names) != len(set(names)) or not {"classes.dex", "AndroidManifest.xml", "resources.arsc", "assets/probe.txt"} <= set(names):
            raise ValueError("missing or duplicate APK fixture content")
    if FRAMEWORK_RESOURCES not in framework["files"]:
        raise ValueError("APK runtime requires stock framework-res.apk; rerun prepare-framework.py")
    return record


def stage_apk_probe(build, tree, digest, tampered_digest):
    for name, expected in [("scarlet-compat.apk", digest), ("scarlet-tampered.apk", tampered_digest)]:
        target = tree / "data/local/tmp" / name
        if target.exists() or target.is_symlink():
            raise ValueError("APK fixture would replace an existing file")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(build / name, target)
        if sha256(target) != expected:
            raise ValueError("APK changed during staging")
