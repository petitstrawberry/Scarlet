"""Verify the stock manager and stage the explicit userspace service-label fixture."""
import json
import shutil
from pathlib import PurePosixPath
from art_inputs import sha256, verify_art_runtime

CONTEXTS = ("manager u:object_r:scarlet_manager_service:s0\n"
            "scarlet.compat.service u:object_r:scarlet_service:s0\n"
            "scarlet.compat.denied u:object_r:scarlet_service:s0\n")

def verify_service_inputs(directory, runtime):
    verify_art_runtime(runtime)
    metadata = json.loads((directory / "service-inputs.json").read_text())
    program = directory / "system/bin/servicemanager"
    if metadata.get("result") != "PREPARED" or metadata.get("base_manifest_sha256") != sha256(runtime / "art-runtime.json"):
        raise ValueError("service input/runtime mismatch")
    if metadata.get("files") != {"system/bin/servicemanager": sha256(program)}:
        raise ValueError("stock servicemanager changed since preparation")
    libraries = metadata.get("runtime_libraries", {})
    if not libraries or "system/lib64/libselinux.so" not in libraries:
        raise ValueError("incomplete stock manager dependency closure")
    for name, digest in libraries.items():
        path = PurePosixPath(name)
        if path.is_absolute() or ".." in path.parts or sha256(runtime / path) != digest:
            raise ValueError("stock manager library mismatch")
    return program


def stage_service_inputs(directory, runtime, target):
    program = verify_service_inputs(directory, runtime)
    destination = target / "system/bin/servicemanager"
    contexts = target / "system/etc/selinux/plat_service_contexts"
    if destination.exists() or contexts.exists():
        raise ValueError("service fixture would overwrite runtime input")
    contexts.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(program, destination)
    contexts.write_text(CONTEXTS)
    if sha256(program) != sha256(destination):
        raise ValueError("stock manager changed during staging")
