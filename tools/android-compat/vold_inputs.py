"""Verified vold user-storage preparation helper for Android boot inputs."""
import json
from pathlib import Path

from art_inputs import sha256

SOURCE = Path(__file__).resolve().parent
C_SOURCE = SOURCE / "vold-prepare-subdirs.c"
BUILD_KIND = "scarlet-vold-subdirs-v1"
BINARY = "system/bin/vold_prepare_subdirs"


def verify_build(build):
    record = json.loads((build / "build.json").read_text())
    binary = build / "vold_prepare_subdirs"
    if (record.get("kind") != BUILD_KIND or record.get("result") != "BUILT" or
            record.get("source_sha256") != sha256(C_SOURCE) or
            record.get("binary_sha256") != sha256(binary) or
            record.get("label_policy") != "host-managed"):
        raise ValueError("changed or incomplete vold subdirectory build")
    return record


def validate_profile(profile):
    if "vold" not in profile.get("host_services", []):
        raise ValueError("vold subdirectory helper requires the stock vold provider")
    if "u:r:vold_prepare_subdirs:s0" not in profile.get("subjects", {}):
        raise ValueError("vold subdirectory helper requires its explicit subject")
    if not any(rule[0] == "u:r:vold:s0" and
               rule[1] == "u:object_r:vold_prepare_subdirs_exec:s0" and
               rule[2] == "u:r:vold_prepare_subdirs:s0" and rule[3] == 0
               for rule in profile.get("exec_transitions", [])):
        raise ValueError("vold subdirectory helper requires its explicit exec transition")


def install(inputs, metadata, build):
    validate_profile(metadata["profile"])
    record = verify_build(build)
    destination = inputs / BINARY
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes((build / "vold_prepare_subdirs").read_bytes())
    destination.chmod(0o755)
    if sha256(destination) != record["binary_sha256"]:
        raise ValueError("changed vold subdirectory helper while staging")
    metadata["vold_subdirs_inputs"] = {
        "build_sha256": sha256(build / "build.json"),
        "source_sha256": record["source_sha256"],
        "binary_sha256": record["binary_sha256"],
        "label_policy": record["label_policy"],
    }


def validate_staged(metadata, inputs):
    validate_profile(metadata["profile"])
    record = metadata.get("vold_subdirs_inputs")
    if not isinstance(record, dict) or set(record) != {
            "build_sha256", "source_sha256", "binary_sha256", "label_policy"}:
        raise ValueError("missing verified vold subdirectory inputs")
    if (record.get("source_sha256") != sha256(C_SOURCE) or
            record.get("label_policy") != "host-managed" or
            sha256(inputs / BINARY) != record.get("binary_sha256")):
        raise ValueError("changed vold subdirectory boot inputs")
