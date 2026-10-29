"""Userspace boot configuration for the pinned stock-init fixture.

The profile is administrator input, never inferred from an ELF or Environment.
It selects factory payloads and explicit Shield grants. Prepared files remain
bound to the original image; successful preparation says nothing about boot.
"""
import json
import posixpath
from pathlib import Path
import re
import shutil

from art_inputs import SDK_SHA256, sha256

MANIFEST = "boot-profile.json"
POLICY = "scarlet/boot/shield.policy"
CONTEXT = re.compile(r"u:(?:r|object_r):[A-Za-z0-9_]+:s0")


def relative_path(name):
    if (not isinstance(name, str) or not name or name.startswith("/") or
            posixpath.normpath(name) != name or name in (".", "..") or name.startswith("../") or
            not re.fullmatch(r"[A-Za-z0-9_./+@-]+", name)):
        raise ValueError("invalid boot input path")
    return name


def guest_alias(source, target):
    relative_path(source)
    if not target or not re.fullmatch(r"[A-Za-z0-9_./+@-]+", target):
        raise ValueError("invalid guest alias")
    resolved = posixpath.normpath(posixpath.join("/", posixpath.dirname(source), target))
    if not resolved.startswith(("/system/", "/system_ext/", "/product/", "/vendor/", "/apex/")):
        raise ValueError("escaping guest alias")
    return target


def validate_profile(profile):
    fields = {"version", "name", "apex_modules", "boot_properties", "bootconfig",
              "subjects", "transitions", "property_grants", "init_imports"}
    if profile.get("version") == 2:
        fields |= {"file_labels", "exec_transitions"}
    if set(profile) != fields or profile.get("version") not in (1, 2):
        raise ValueError("unsupported boot profile")
    if not re.fullmatch(r"[a-z0-9-]+", profile["name"]):
        raise ValueError("invalid profile name")
    modules = profile["apex_modules"]
    if not modules or len(set(modules)) != len(modules) or any(
            not re.fullmatch(r"com\.android\.[A-Za-z0-9_.]+", name) for name in modules):
        raise ValueError("invalid factory APEX selection")
    if profile["boot_properties"].get("ro.apex.updatable") != "false":
        raise ValueError("flattened profile requires non-updatable APEX")
    for name, value in profile["boot_properties"].items():
        if not re.fullmatch(r"[A-Za-z0-9_.-]+", name) or not isinstance(value, str) or any(c in value for c in "\n\r\0$"):
            raise ValueError("invalid boot property")
    for name, value in profile["bootconfig"].items():
        if not re.fullmatch(r"androidboot\.[A-Za-z0-9_.]+", name) or not re.fullmatch(r"[A-Za-z0-9_./-]+", value):
            raise ValueError("invalid bootconfig entry")
    subjects = profile["subjects"]
    if len(subjects) > 64 or len(set(subjects.values())) != len(subjects) or any(
            not CONTEXT.fullmatch(label) or not label.startswith("u:r:") or
            type(identifier) is not int or not 1 <= identifier < 100 for label, identifier in subjects.items()):
        raise ValueError("invalid profile subjects")
    if subjects.get("u:r:init:s0") != 1:
        raise ValueError("init subject must be explicit")
    seen = set()
    for transition in profile["transitions"]:
        source, target, principal = transition
        key = (source, target, principal)
        if (source not in subjects or target not in subjects or source == target or
                type(principal) is not int or not 0 <= principal <= 0xffffffff or key in seen):
            raise ValueError("invalid profile transition")
        seen.add(key)
    for subject, targets in profile["property_grants"].items():
        if subject not in subjects or len(set(targets)) != len(targets) or any(
                not CONTEXT.fullmatch(label) or not label.startswith("u:object_r:") for label in targets):
            raise ValueError("invalid property grant")
    resource_labels = set(profile.get("file_labels", {}).values())
    for path, label in profile.get("file_labels", {}).items():
        if not path.startswith(("/system/", "/apex/")) or not CONTEXT.fullmatch(label) or not label.startswith("u:object_r:"):
            raise ValueError("invalid file-label binding")
        relative_path(path[1:])
    seen_exec = set()
    for source, resource, target, principal in profile.get("exec_transitions", []):
        key = (source, resource, principal)
        if source not in subjects or target not in subjects or resource not in resource_labels or type(principal) is not int or not 0 <= principal <= 0xffffffff or key in seen_exec:
            raise ValueError("invalid file exec transition")
        seen_exec.add(key)
    for path in profile["init_imports"]:
        if not path.startswith("/") or not relative_path(path[1:]).endswith(".rc"):
            raise ValueError("invalid init import")
    return profile


def shield_policy(profile, contexts):
    validate_profile(profile)
    labels = set()
    for text in contexts:
        for line in text.splitlines():
            fields = line.split("#", 1)[0].split()
            if not fields:
                continue
            if len(fields) < 2 or not CONTEXT.fullmatch(fields[1]):
                raise ValueError("unsupported stock property context")
            labels.add(fields[1])
    grants = profile["property_grants"]
    if any(target not in labels for targets in grants.values() for target in targets):
        raise ValueError("property grant is absent from verified contexts")
    if len(labels | set(profile.get("file_labels", {}).values())) + len(profile["subjects"]) > 512:
        raise ValueError("profile exceeds native label bound")
    labels |= set(profile.get("file_labels", {}).values())
    identifiers = dict(profile["subjects"])
    identifiers.update((label, index) for index, label in enumerate(sorted(labels), 100))
    lines = ["scarlet-android-boot-policy-v" + str(profile["version"])]
    lines += [f"label {identifier} {label}" for label, identifier in sorted(identifiers.items(), key=lambda item: item[1])]
    lines += [f"transition {identifiers[source]} {identifiers[target]} {principal}"
              for source, target, principal in profile["transitions"]]
    lines += [f"property-set {identifiers[source]} {identifiers[target]}"
              for source, targets in sorted(grants.items()) for target in sorted(targets)]
    lines += [f"file-exec {identifiers[source]} {identifiers[resource]} {identifiers[target]} {principal}"
              for source, resource, target, principal in profile.get("exec_transitions", [])]
    lines += [f"file-label {identifiers[label]} {path}" for path, label in sorted(profile.get("file_labels", {}).items())]
    return "\n".join(lines) + "\n"


def verify_boot_inputs(inputs, runtime, stock):
    metadata = json.loads((inputs / MANIFEST).read_text())
    if (metadata.get("kind") != "scarlet-android-boot-inputs-v1" or metadata.get("result") != "PREPARED" or
            metadata.get("sdk_sha256") != SDK_SHA256 or
            metadata.get("runtime_manifest_sha256") != sha256(runtime / "art-runtime.json") or
            metadata.get("stock_manifest_sha256") != sha256(stock / "stock-inputs.json")):
        raise ValueError("incomplete or mismatched boot inputs")
    validate_profile(metadata["profile"])
    files, links, dirs = metadata["files"], metadata["symlinks"], set(metadata["directories"])
    all_paths = set(files) | set(links) | dirs
    for name in all_paths:
        relative_path(name)
        if name == MANIFEST or any(str(parent) in links for parent in Path(name).parents):
            raise ValueError("input alias is an ancestor")
    if set(files) & (set(links) | dirs) or set(links) & dirs:
        raise ValueError("conflicting boot inputs")
    for name, target in links.items():
        guest_alias(name, target)
    actual_files, actual_dirs = set(), set()
    for path in inputs.rglob("*"):
        name = str(path.relative_to(inputs))
        if path.is_symlink():
            raise ValueError("prepared boot tree must not contain host symlinks")
        if path.is_dir():
            actual_dirs.add(name)
        elif name != MANIFEST:
            if name not in files or sha256(path) != files[name]:
                raise ValueError(f"changed or unrecorded boot file: {name}")
            actual_files.add(name)
    if actual_files != set(files) or actual_dirs != dirs:
        raise ValueError("missing boot inputs")
    contexts = [(stock / name).read_text() for name in metadata["property_context_files"]]
    if (inputs / POLICY).read_text() != shield_policy(metadata["profile"], contexts):
        raise ValueError("boot Shield policy differs from explicit profile")
    return metadata


def stage_boot_inputs(inputs, runtime, stock, root):
    metadata = verify_boot_inputs(inputs, runtime, stock)
    # Validate every destination before modifying any file. The prepared overlay
    # must never write through an alias from the base runtime or another input.
    for name in sorted(set(metadata["files"]) | set(metadata["symlinks"]) | set(metadata["directories"])):
        path = root / name
        if any(parent.is_symlink() for parent in (path, *path.parents) if parent != root and root in parent.parents):
            raise ValueError(f"boot destination contains an alias: {name}")
        if path.exists() and name in metadata["files"]:
            digest = sha256(path)
            expected = metadata["replacements"].get(name, metadata["files"][name])
            if digest != expected:
                raise ValueError(f"boot input conflicts with base: {name}")
        elif path.exists() and name in metadata["symlinks"]:
            raise ValueError(f"boot alias conflicts with base: {name}")
    for name in sorted(metadata["directories"], key=lambda value: len(Path(value).parts)):
        (root / name).mkdir(exist_ok=True, parents=True)
    for name, digest in metadata["files"].items():
        shutil.copy2(inputs / name, root / name)
        if sha256(root / name) != digest:
            raise ValueError("boot input changed while staging")
    for name, target in metadata["symlinks"].items():
        (root / name).symlink_to(target)
    return metadata
