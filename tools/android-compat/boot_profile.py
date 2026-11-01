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
KEYSTORE_PERMISSIONS = ["add_auth", "clear_ns", "get_state", "list", "lock", "reset", "unlock",
    "change_user", "change_password", "clear_uid", "get_auth_token", "early_boot_ended",
    "report_off_body", "pull_metrics", "delete_all_keys", "get_attestation_key"]
CONTEXT = re.compile(r"u:(?:r|object_r):[A-Za-z0-9_]+:s0(?::c(?:0|[1-9][0-9]*)(?:,c(?:0|[1-9][0-9]*))*)?")


def valid_context(name):
    if not isinstance(name,str) or len(name)>255 or not CONTEXT.fullmatch(name):return False
    fields=name.split(":")
    if len(fields)==4:return True
    values=[int(category[1:]) for category in fields[4].split(",")]
    return all(value<=1023 for value in values) and values==sorted(set(values))


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
        if "resource_groups" in profile: fields.add("resource_groups")
        if "file_policy" in profile: fields.add("file_policy")
        if "service_grants" in profile: fields.add("service_grants")
        if "keystore_grants" in profile: fields.add("keystore_grants")
        if "socket_labels" in profile: fields.add("socket_labels")
        if "vintf" in profile: fields.add("vintf")
        if "host_services" in profile: fields.add("host_services")
        if "package_set" in profile: fields.add("package_set")
    if set(profile) != fields or profile.get("version") not in (1, 2):
        raise ValueError("unsupported boot profile")
    if not re.fullmatch(r"[a-z0-9-]+", profile["name"]):
        raise ValueError("invalid profile name")
    if "vintf" in profile:
        from vintf_inputs import validate_selection
        validate_selection(profile["vintf"])
    from host_services import validate_selection as validate_host_services
    validate_host_services(profile)
    from desktop_inputs import validate_selection as validate_packages
    validate_packages(profile)
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
    if "resource_groups" in profile:
        resources = profile["resource_groups"]
        if resources not in ({"cgroup2_root": "/sys/fs/cgroup", "controllers": []}, {"cgroup2_root": "/sys/fs/cgroup", "controllers": [], "cpuset_root": "/dev/cpuset"}):
            raise ValueError("unsupported resource controller configuration")
    subjects = profile["subjects"]
    if len(subjects) > 64 or len(set(subjects.values())) != len(subjects) or any(
            not valid_context(label) or not label.startswith("u:r:") or
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
                not valid_context(label) or not label.startswith("u:object_r:") for label in targets):
            raise ValueError("invalid property grant")
    socket_labels=profile.get("socket_labels",[])
    if len(socket_labels)>4096 or len({tuple(rule) for rule in socket_labels})!=len(socket_labels) or any(len(rule)!=2 or rule[0] not in subjects or not valid_context(rule[1]) or rule[1] not in subjects and not rule[1].startswith("u:object_r:") for rule in socket_labels):raise ValueError("invalid socket label delegation")
    seen_services=set()
    for source,target,permissions in profile.get("service_grants",[]):
        if source not in subjects or not valid_context(target) or not (target.startswith("u:object_r:") or target in subjects) or type(permissions) is not int or not 1<=permissions<=7 or (source,target) in seen_services:
            raise ValueError("invalid service grant")
        seen_services.add((source,target))
    if len(seen_services)>4096:raise ValueError("service rule bound")
    resource_labels = set(profile.get("file_labels", {}).values())
    for path, label in profile.get("file_labels", {}).items():
        if not path.startswith(("/system/", "/apex/", "/vendor/")) or not valid_context(label) or not label.startswith("u:object_r:"):
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
    if "file_policy" in profile: file_rules(profile)
    if not isinstance(profile.get("keystore_grants", []), list) or len(profile.get("keystore_grants", [])) > 64:
        raise ValueError("invalid keystore grants")
    for rule in profile.get("keystore_grants", []):
        if (not isinstance(rule, list) or len(rule) != 3 or rule[0] not in subjects or
                rule[1] not in subjects or not isinstance(rule[2], list) or not rule[2] or
                any(name not in KEYSTORE_PERMISSIONS for name in rule[2]) or
                len(set(rule[2])) != len(rule[2])):
            raise ValueError("invalid keystore grant")
    return profile


def file_rules(profile):
    config=profile["file_policy"]
    if set(config)!={"default","create","relabel","property_areas"}: raise ValueError("invalid file policy fields")
    labels={config["default"]};create=set();relabel=set()
    def contexts(values):
        if not isinstance(values,list) or not values or len(set(values))!=len(values) or any(not isinstance(value,str) or not valid_context(value) or not value.startswith("u:object_r:") for value in values): raise ValueError("invalid file labels")
        labels.update(values);return values
    contexts([config["default"]])
    for spec in config["create"]+config["relabel"]:
        is_create="parents" in spec
        expected={"source","parents","targets","kinds","default"} if is_create else {"source","from","targets","kinds"}
        if set(spec)!=expected or spec["source"] not in profile["subjects"]: raise ValueError("invalid file subject or rule")
        parents=contexts(spec["parents" if is_create else "from"]);targets=contexts(spec["targets"]);kinds=spec["kinds"]
        if not isinstance(kinds,list) or not kinds or len(set(kinds))!=len(kinds) or any(type(kind)!=int or not 1<=kind<=7 for kind in kinds): raise ValueError("invalid native file kind")
        if is_create and (type(spec["default"])!=bool or spec["default"] and len(targets)!=1): raise ValueError("ambiguous create default")
        for parent in parents:
            for target in targets:
                for kind in kinds:
                    if is_create: create.add((spec["source"],parent,target,kind,int(spec["default"])))
                    elif parent!=target: relabel.add((spec["source"],parent,target,kind))
    seen_areas=set()
    for area in config["property_areas"]:
        if not isinstance(area,dict) or set(area)!={"source","targets"}: raise ValueError("invalid property area configuration")
        source=area["source"]
        if source not in profile["subjects"] or source in seen_areas: raise ValueError("unknown or duplicate property area subject")
        seen_areas.add(source)
        parents=contexts([config["default"],"u:object_r:properties_device:s0"])
        for target in contexts(area["targets"]):
            for parent in parents:
                create.add((source,parent,target,1,0));relabel.add((source,parent,target,1))
    if len(create)>4096 or len(relabel)>4096: raise ValueError("file rule bound")
    defaults={}
    for source,parent,target,kind,default in create:
        if default and (source,parent,kind) in defaults: raise ValueError("ambiguous file create default")
        if default: defaults[(source,parent,kind)]=target
    return labels,sorted(create),sorted(relabel)


def shield_policy(profile, contexts):
    validate_profile(profile)
    labels = set()
    for text in contexts:
        for line in text.splitlines():
            fields = line.split("#", 1)[0].split()
            if not fields:
                continue
            if len(fields) < 2 or not valid_context(fields[1]):
                raise ValueError("unsupported stock property context")
            labels.add(fields[1])
    grants = profile["property_grants"]
    if any(target not in labels for targets in grants.values() for target in targets):
        raise ValueError("property grant is absent from verified contexts")
    file_labels,create,relabel=file_rules(profile) if "file_policy" in profile else (set(),[],[])
    labels |= file_labels
    labels |= {target for _,target,_ in profile.get("service_grants",[])}
    labels |= {target for _,target in profile.get("socket_labels",[])}
    labels -= set(profile["subjects"])
    # Mirrors the native Shield label bound (see kernel/src/shield/mod.rs).
    if len(labels | set(profile.get("file_labels", {}).values())) + len(profile["subjects"]) > 1024:
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
    lines += [f"socket-create {identifiers[source]} {identifiers[target]}" for source,target in profile.get("socket_labels",[])]
    lines += [f"service-allow {identifiers[source]} {identifiers[target]} {permissions}" for source,target,permissions in profile.get("service_grants",[])]
    lines += [f"keystore-allow {identifiers[source]} {identifiers[target]} " +
              str(sum(1 << KEYSTORE_PERMISSIONS.index(name) for name in permissions))
              for source, target, permissions in profile.get("keystore_grants", [])]
    lines += [f"file-exec {identifiers[source]} {identifiers[resource]} {identifiers[target]} {principal}"
              for source, resource, target, principal in profile.get("exec_transitions", [])]
    lines += [f"file-label {identifiers[label]} {path}" for path, label in sorted(profile.get("file_labels", {}).items())]
    if "file_policy" in profile:
        lines += [f"file-default {identifiers[profile['file_policy']['default']]}"]
        lines += [f"file-create {identifiers[source]} {identifiers[parent]} {identifiers[target]} {kind} {default}" for source,parent,target,kind,default in create]
        lines += [f"file-relabel {identifiers[source]} {identifiers[old]} {identifiers[target]} {kind}" for source,old,target,kind in relabel]
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
    if metadata.get("vold_subdirs_inputs") is not None:
        from vold_inputs import validate_staged as validate_vold_subdirs
        validate_vold_subdirs(metadata, inputs)
    return metadata


def stage_boot_inputs(inputs, runtime, stock, root):
    metadata = verify_boot_inputs(inputs, runtime, stock)
    from desktop_inputs import validate_staged as validate_packages
    validate_packages(metadata)
    from vintf_inputs import validate_staged
    validate_staged(metadata)
    from host_services import validate_staged as validate_host_services
    validate_host_services(metadata)
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
