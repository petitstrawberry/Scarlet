"""Verified Android user-0 unlock helper for the hosted boot profile."""
import json
from pathlib import Path

from art_inputs import sha256, validate_dex

SOURCE = Path(__file__).resolve().parent
JAVA_SOURCE = SOURCE / "framework/ScarletUserUnlock.java"
BUILD_KIND = "scarlet-user-unlock-v1"
RUNNER = "system/bin/scarlet-user-unlock"
DEX = "data/local/tmp/scarlet-user-unlock.dex"


def service_block():
    return ("\nservice scarlet-user-unlock /system/bin/scarlet-user-unlock "
            "-Xint -Xusejit:false -Ximage:/system/nonexistent/boot.art "
            "-Xnoimage-dex2oat -Xms16m -Xmx64m /system/bin --application "
            "ScarletUserUnlock\n"
            "    class core\n"
            "    user system\n"
            "    group system\n"
            "    setenv CLASSPATH /data/local/tmp/scarlet-user-unlock.dex\n"
            "    oneshot\n"
            "    disabled\n\n"
            "on property:init.svc.zygote=running\n"
            "    start scarlet-user-unlock\n")


def verify_build(build):
    record = json.loads((build / "build.json").read_text())
    if (record.get("kind") != BUILD_KIND or record.get("result") != "BUILT" or
            record.get("source_sha256") != sha256(JAVA_SOURCE) or
            record.get("dex_sha256") != validate_dex(build / "classes.dex")):
        raise ValueError("changed or incomplete user-unlock build")
    return record


def validate_profile(profile):
    if "u:r:user_unlock:s0" not in profile.get("subjects", {}):
        raise ValueError("user-unlock helper requires its explicit subject")
    if not any(rule[0] == "u:r:init:s0" and
               rule[1] == "u:object_r:user_unlock_exec:s0" and
               rule[2] == "u:r:user_unlock:s0" and rule[3] == 1000
               for rule in profile.get("exec_transitions", [])):
        raise ValueError("user-unlock helper requires its explicit exec transition")


def install(inputs, metadata, build):
    validate_profile(metadata["profile"])
    record = verify_build(build)
    app_process = inputs / "system/bin/app_process64"
    if not app_process.is_file():
        raise ValueError("user-unlock helper requires verified app_process64")
    destination = inputs / RUNNER
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(app_process.read_bytes())
    destination.chmod(0o755)
    dex = inputs / DEX
    dex.parent.mkdir(parents=True, exist_ok=True)
    dex.write_bytes((build / "classes.dex").read_bytes())
    if validate_dex(dex) != record["dex_sha256"]:
        raise ValueError("changed user-unlock DEX while staging")
    apply_service(inputs)
    metadata["user_unlock_inputs"] = {
        "build_sha256": sha256(build / "build.json"),
        "source_sha256": record["source_sha256"],
        "runner_sha256": sha256(destination),
        "dex_sha256": record["dex_sha256"],
        "main_class": record["main_class"],
        "ce_property": "sys.user.0.ce_available",
    }


def apply_service(inputs):
    path = inputs / "scarlet/boot/init.rc"
    text = path.read_text()
    marker = "service scarlet-user-unlock "
    if marker in text:
        return
    path.write_text(text.rstrip("\n") + "\n" + service_block())


def validate_staged(metadata, inputs):
    record = metadata.get("user_unlock_inputs")
    validate_profile(metadata["profile"])
    if not isinstance(record, dict) or set(record) != {
            "build_sha256", "source_sha256", "runner_sha256", "dex_sha256",
            "main_class", "ce_property"}:
        raise ValueError("missing verified user-unlock inputs")
    if (record.get("source_sha256") != sha256(JAVA_SOURCE) or
            record.get("main_class") != JAVA_SOURCE.stem or
            record.get("ce_property") != "sys.user.0.ce_available" or
            sha256(inputs / RUNNER) != record.get("runner_sha256") or
            sha256(inputs / "system/bin/app_process64") != record.get("runner_sha256") or
            validate_dex(inputs / DEX) != record.get("dex_sha256")):
        raise ValueError("changed user-unlock boot inputs")
    if "service scarlet-user-unlock " not in (inputs / "scarlet/boot/init.rc").read_text():
        raise ValueError("missing user-unlock init service")
