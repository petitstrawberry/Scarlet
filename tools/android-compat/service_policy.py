"""Translate compiled Android service permissions, without interpreting CIL."""
import hashlib
import json
import re
from pathlib import Path

GENERIC_MAGIC = "scarlet-android-generic-exec-policy-v1"
# platform/system/core 83366e7e7a1556ab6cff117673bb9f17b973de99,
# libcutils/include/private/android_filesystem_config.h (android-13.0.0_r2).
AID_HEADER_SHA256 = "d6b164463418ba89aaf4ff9ce1367268fc0c6a6944b647c44905db65a40c0e5b"
# Same immutable SDK extents as stock_art.PARTITIONS; verified before reading.
GENERIC_PARTITIONS = {
    "system": "cea78da8aafa4ef03c4fd993c0b1df2592286f7e5752e66a4aead7e439457217",
    "system_ext": "03106097e796f6e9e72d75c5a3fd4a395e197e7d59a04022a604d6d8a2885fe5",
    "product": "76dd3a2369c0da1b88a851432614688af33058f159fdf1434db4391693ba00d7",
    "vendor": "4222de4d8d0839d4550a15835cbe2fd7c42af90996357e6338e5605dbf3c2c52",
}

MAGIC = "scarlet-android-service-policy-v1"
ACCESS_MAGIC = "scarlet-android-access-policy-v2"
CLASSES = ("service_manager", "hwservice_manager")
PERMISSIONS = {"add": 1, "find": 2, "list": 4}
# /vendor/etc/selinux/precompiled_sepolicy in the SDK pinned by art_inputs.py.
SDK_POLICY_SHA256 = "2be08e2a2e93d22c6a1c5e4e32fa4d571ecb07695877cdcfe0c3a9f48ba9234c"
SDK_POLICY_VERSION = 30
MAX_NATIVE_LABELS = 2048


def generic_file_relabels(policy, setools, context_sources, configured_contexts=(), source="init"):
    """Canonical s0 restorecon grants for an initially unlabeled View.

    Both relabelfrom and relabelto must be present in the pinned TE policy.
    Targets come from actual file_contexts and explicit management inputs.
    App-specific category labels remain the package manager's responsibility.
    """
    if source not in ("init", "installd"):
        raise ValueError("unsupported stock label restorer")
    contexts = {":".join(context.split(":")[:4]) for context in configured_contexts
                if re.fullmatch(r"u:object_r:[A-Za-z0-9_]+:s0(?::c[0-9]+(?:,c[0-9]+)*)?", context)}
    for data in context_sources.values():
        for line in data.decode().splitlines():
            fields = line.split("#", 1)[0].split()
            if fields and re.fullmatch(r"u:object_r:[A-Za-z0-9_]+:s0", fields[-1]):
                contexts.add(fields[-1])
    selected = {context.split(":")[2]: context for context in contexts}
    grants, evidence = set(), set()
    for kind, name in enumerate(("file", "dir", "lnk_file", "chr_file", "blk_file", "sock_file", "fifo_file"), 1):
        for constraint in setools.ConstraintQuery(policy, tclass=[name], perms=["relabelfrom", "relabelto"]).results():
            expression = " ".join(str(constraint.expression).split())
            if (str(constraint.ruletype) != "mlsconstrain" or expression !=
                    "( l2 == h2 and ( l1 == l2 ) or ( t1 == mlstrustedsubject ) )"):
                raise ValueError("unsupported init file relabel constraint: " + str(constraint))
        if list(setools.ConstraintQuery(policy, ruletype=["validatetrans", "mlsvalidatetrans"], tclass=[name]).results()):
            raise ValueError("init file validatetrans requires a different translation")
        origins = list(setools.TERuleQuery(policy, ruletype=["allow"], source=source,
            target="unlabeled", tclass=[name], perms=["relabelfrom"]).results())
        for rule in origins:
            _unconditional(rule, setools)
        if not origins:
            continue
        for rule in setools.TERuleQuery(policy, ruletype=["allow"], source=source,
                tclass=[name], perms=["relabelto"]).results():
            targets = {str(target) for target in rule.target.expand()} & selected.keys()
            if not targets:
                continue
            _unconditional(rule, setools)
            evidence.update(map(str, origins))
            evidence.add(str(rule))
            grants.update((f"u:r:{source}:s0", "u:object_r:unlabeled:s0", selected[target], kind)
                          for target in targets if target != "unlabeled")
    return [list(rule) for rule in sorted(grants)], sorted(evidence)


def policy_version(metadata):
    """Return the mapped version of the pinned Android source policy."""
    if not isinstance(metadata, dict):
        raise ValueError("service policy metadata")
    version = metadata.get("policy_version")
    if type(version) is not int or version != SDK_POLICY_VERSION:
        raise ValueError("service policy version")
    return version


def validate_metadata(metadata, text, source=None):
    """Bind a manager manifest to its exact generated and source bytes."""
    if not isinstance(metadata, dict):
        raise ValueError("service policy metadata")
    domains, rules = validate(text)
    version = policy_version(metadata)
    if (metadata.get("kind") != text.splitlines()[0] or
            metadata.get("text_sha256") != hashlib.sha256(text.encode()).hexdigest() or
            type(metadata.get("domains")) is not int or metadata["domains"] != len(domains) or
            type(metadata.get("rules")) is not int or metadata["rules"] != len(rules)):
        raise ValueError("changed service policy metadata")
    if metadata.get("source_sha256") != SDK_POLICY_SHA256:
        raise ValueError("service policy source")
    if source is not None and hashlib.sha256(source).hexdigest() != SDK_POLICY_SHA256:
        raise ValueError("changed service policy source")
    return version


def export(policy, setools, additional_domains=(), properties=False):
    # A type-only Shield rule cannot represent conditional or MLS constraints.
    # Reject those inputs instead of silently granting a superset of access.
    selected = CLASSES + (("property_service",) if properties else ())
    if list(setools.ConstraintQuery(policy, tclass=selected).results()):
        raise ValueError("service class constraints require a different translation")
    classes = {str(c): c for c in policy.classes() if str(c) in selected}
    if set(classes) != set(selected) or any(set(c.perms) != ({"set"} if name == "property_service" else set(PERMISSIONS)) for name, c in classes.items()):
        raise ValueError("unsupported Android service permission vocabulary")
    domains = {str(t) for t in policy.lookup_typeattr("domain").expand()}
    apps = {str(t) for t in policy.lookup_typeattr("appdomain").expand()}
    apps.update(additional_domains)
    if not apps or not apps <= domains:
        raise ValueError("invalid app domain inventory")
    rules = {}
    target_classes = {}
    for rule in setools.TERuleQuery(policy, ruletype=["allow"], tclass=selected).results():
        try:
            rule.conditional
        except setools.exception.RuleNotConditional:
            pass
        else:
            raise ValueError("conditional service rules require a different translation")
        sources = {str(t) for t in rule.source.expand()} & apps
        if not sources:
            continue
        native_class = 2 if str(rule.tclass) == "property_service" else 1
        permissions = {"set": 1} if native_class == 2 else PERMISSIONS
        if not set(rule.perms) <= set(permissions):
            raise ValueError("unsupported service permission")
        mask = sum(permissions[p] for p in rule.perms)
        for target in rule.target.expand():
            name = str(target)
            # The current Android ABI maps both classes to Shield's service
            # class. Only accept disjoint target types, so the mapping is exact.
            previous = target_classes.setdefault((native_class, name), str(rule.tclass))
            if previous != str(rule.tclass):
                raise ValueError("service classes overlap on target " + name)
            label = f"u:{'r' if name in domains else 'object_r'}:{name}:s0"
            for source in sources:
                key = (source, label, native_class)
                rules[key] = rules.get(key, 0) | mask
    lines = [ACCESS_MAGIC if properties else MAGIC] + [f"domain {name}" for name in sorted(apps)]
    lines += [f"allow {source} {target} " + (f"{kind} " if properties else "") + str(mask)
              for (source, target, kind), mask in sorted(rules.items())]
    text = "\n".join(lines) + "\n"
    validate(text)
    return text


def validate(text):
    if len(text.encode()) > 1024 * 1024 or not text.endswith("\n"):
        raise ValueError("service policy size or termination")
    lines = text.splitlines()
    if not lines or lines[0] not in (MAGIC, ACCESS_MAGIC):
        raise ValueError("service policy version")
    domains, rules = set(), {}
    for line in lines[1:]:
        fields = line.split()
        if len(fields) == 2 and fields[0] == "domain":
            name = fields[1]
            if not re.fullmatch(r"[A-Za-z0-9_]+", name) or name in domains:
                raise ValueError("service policy domain")
            domains.add(name)
        elif len(fields) == (5 if lines[0] == ACCESS_MAGIC else 4) and fields[0] == "allow":
            _, source, target, *values = fields
            kind, permissions = values if lines[0] == ACCESS_MAGIC else ("1", values[0])
            if (source not in domains or
                    not re.fullmatch(r"u:(r|object_r):[A-Za-z0-9_]+:s0", target) or
                    kind not in {"1", "2"} or kind == "2" and permissions != "1" or
                    permissions not in {"1", "2", "3", "4", "5", "6", "7"} or
                    (source, target, int(kind)) in rules):
                raise ValueError("service policy rule")
            rules[source, target, int(kind)] = int(permissions)
        else:
            raise ValueError("service policy record")
    if not domains or len(domains) > 256 or len(rules) > 16384:
        raise ValueError("service policy count")
    return domains, rules


def verify(directory):
    directory = Path(directory)
    metadata = json.loads((directory / "service-policy.json").read_text())
    path = directory / "service-policy.txt"
    text = path.read_text()
    validate_metadata(metadata, text)
    if "generic_exec_manifest_sha256" in metadata:
        generic = validate_generic(json.loads((directory / "generic-service-policy.json").read_text()))
        if generic["manifest_sha256"] != metadata["generic_exec_manifest_sha256"]:
            raise ValueError("generic and service export selection differ")
    return metadata


def apply_boot_policy(boot, service_text, metadata=None):
    """Use the same compiled rules for existing boot subjects and future apps."""
    domains, grants = validate(service_text)
    version = validate_metadata(metadata, service_text) if metadata is not None else None
    labels, identifiers = {}, set()
    lines = boot.splitlines()
    if not lines or lines[0] not in ("scarlet-android-boot-policy-v1", "scarlet-android-boot-policy-v2"):
        raise ValueError("boot policy version")
    for line in lines:
        fields = line.split()
        if fields[:1] == ["label"]:
            if len(fields) != 3:
                raise ValueError("boot label syntax")
            identifier, name = int(fields[1]), fields[2]
            if name in labels or identifier in identifiers:
                raise ValueError("duplicate boot label")
            labels[name] = identifier
            identifiers.add(identifier)
    from boot_profile import valid_context
    sources = {}
    for name, identifier in labels.items():
        if name.startswith("u:r:") and valid_context(name):
            domain = name.split(":")[2]
            if domain in domains:
                sources.setdefault(domain, set()).add(identifier)
    replaced = {identifier for identifiers in sources.values() for identifier in identifiers}
    replaced_kinds = ("service-allow ", "property-set ") if service_text.startswith(ACCESS_MAGIC + "\n") else ("service-allow ",)
    kept = [line for line in lines if not (line.startswith(replaced_kinds) and int(line.split()[1]) in replaced)]
    if version is not None:
        kept = [line for line in kept if not line.startswith("policy-version ")]
        kept.insert(1, f"policy-version {version}")
    next_id = max(identifiers, default=0) + 1
    added, rules = [], []
    for (source, target, kind), mask in sorted(grants.items()):
        if source not in sources:
            continue
        if target not in labels:
            labels[target] = next_id
            added.append(f"label {next_id} {target}")
            next_id += 1
        for identifier in sorted(sources[source]):
            rules.append(f"service-allow {identifier} {labels[target]} {mask}" if kind == 1 else f"property-set {identifier} {labels[target]}")
    if len(labels) > MAX_NATIVE_LABELS or sum(line.startswith(("service-allow ", "property-set ", "socket-create ", "keystore-allow ")) for line in kept) + len(rules) > 16384:
        raise ValueError("compiled service policy exceeds native policy bounds")
    return "\n".join(kept + added + rules) + "\n"


def _digest(data):
    return hashlib.sha256(data).hexdigest()


def _unconditional(rule, setools):
    try:
        rule.conditional
    except setools.exception.RuleNotConditional:
        return
    raise ValueError("conditional generic exec rule: " + str(rule))


def generic_transition(policy, setools, executable, seclabel=None):
    """Resolve only s0 init launches, requiring actual TE authorization."""
    if not re.fullmatch(r"u:object_r:[A-Za-z0-9_]+:s0", executable):
        raise ValueError("generic executable context")
    file_type = executable.split(":")[2]
    policy.lookup_type(file_type)
    rules = list(setools.TERuleQuery(policy, ruletype=["type_transition"],
        source="init", target=file_type, tclass=["process"]).results())
    for rule in rules:
        _unconditional(rule, setools)
    if seclabel is None:
        targets = {str(rule.default) for rule in rules}
        if len(targets) != 1:
            raise ValueError("missing or ambiguous init process transition: " + file_type)
        target = targets.pop()
    else:
        if not re.fullmatch(r"u:r:[A-Za-z0-9_]+:s0", seclabel):
            raise ValueError("generic explicit seclabel")
        target = seclabel.split(":")[2]
    if target not in {str(t) for t in policy.lookup_typeattr("domain").expand()}:
        raise ValueError("generic target is not a domain")
    if target not in {str(t) for t in policy.lookup_role("r").types()}:
        raise ValueError("generic target role")
    # The stock transition MLS constraint is true for equal single-level s0
    # contexts. Never discard arbitrary constraints or validatetrans rules.
    for rule in setools.ConstraintQuery(policy, tclass=["process"], perms=["transition"]).results():
        expression = " ".join(str(rule.expression).split())
        if (str(rule.ruletype) != "mlsconstrain" or expression !=
                "h1 == h2 and ( l1 == l2 ) or ( t1 == mlstrustedsubject )"):
            raise ValueError("unsupported generic transition constraint: " + str(rule))
    if list(setools.ConstraintQuery(policy, ruletype=["validatetrans", "mlsvalidatetrans"],
                                   tclass=["process"]).results()):
        raise ValueError("generic validatetrans requires a different translation")
    evidence = [str(rule) for rule in rules] if seclabel is None else []
    checks = [("init", file_type, "file", "execute")]
    if target != "init":
        checks += [("init", target, "process", "transition"),
                   (target, file_type, "file", "entrypoint")]
    else:
        checks += [("init", file_type, "file", "execute_no_trans")]
    for source, destination, kind, permission in checks:
        allows = list(setools.TERuleQuery(policy, ruletype=["allow"], source=source,
            target=destination, tclass=[kind], perms=[permission]).results())
        if not allows:
            raise ValueError(f"missing generic {kind} {permission}: {source} {destination}")
        for rule in allows:
            _unconditional(rule, setools)
        evidence += [str(rule) for rule in allows]
    return "u:r:" + target + ":s0", sorted(set(evidence))


def generic_uids(passwd, header):
    """Use stock passwd and a hash-pinned built-in AID table, never host NSS."""
    if _digest(header) != AID_HEADER_SHA256:
        raise ValueError("generic AID header source")
    aids = {name.lower(): int(value) for name, value in re.findall(
        r"^#define AID_([A-Z0-9_]+)\s+([0-9]+)\b", header.decode(), re.M)}
    for old, new in (("media_codec", "mediacodec"), ("media_ex", "mediaex"),
                     ("media_drm", "mediadrm")):
        aids[new] = aids.pop(old)
    for line in passwd.decode().splitlines():
        if not line or line.startswith("#"):
            continue
        fields = line.split(":")
        if len(fields) != 7 or not fields[2].isdigit():
            raise ValueError("generic stock passwd record")
        name, uid = fields[0], int(fields[2])
        if name in aids and aids[name] != uid:
            raise ValueError("conflicting generic UID: " + name)
        aids[name] = uid
    return aids


def _generic_graph(root, metadata):
    """Read the same effective imports/APEX overrides as init assembly."""
    from aosp_inventory import parse_rc
    from stock_packages import _versioned_apex_configs
    properties = metadata["profile"]["boot_properties"]
    configs, seen, services = {}, set(), {}
    expected = dict(metadata["files"])
    expected.update(metadata.get("stock_init", {}).get("files", {}))

    def expand(value):
        def replace(match):
            name, _, default = match[1].partition(":-")
            return properties.get(name, default if ":-" in match[1] else match[0])
        value = re.sub(r"\$\{([^}]+)\}", replace, value)
        if "${" in value:
            raise ValueError("unresolved generic init path: " + value)
        return value

    def read(relative, apex=False):
        from boot_profile import relative_path
        relative_path(relative)
        if len(seen) >= 256:
            raise ValueError("generic init graph bound")
        if relative in seen:
            raise ValueError("duplicate generic RC import: " + relative)
        seen.add(relative)
        path = root / relative
        if path.is_symlink() or not path.is_file():
            raise ValueError("missing generic RC: " + relative)
        data = path.read_bytes()
        if expected.get(relative) != _digest(data):
            raise ValueError("changed generic RC: " + relative)
        configs[relative] = _digest(data)
        sections = parse_rc(data.decode())
        for section in sections:
            words = section["header"]
            if words[0] == "service":
                name = words[1]
                options = [item["args"] for item in section["commands"]]
                flags = {option[0] for option in options}
                if name in services and not (apex and "override" in flags and
                        "updatable" in services[name]["flags"]):
                    raise ValueError("duplicate generic service: " + name)
                def option(key, default):
                    values = [value for value in options if value[0] == key]
                    if len(values) > 1 or values and len(values[0]) != 2:
                        raise ValueError("ambiguous generic service " + key)
                    return values[0][1] if values else default
                sockets = []
                for socket in (value for value in options if value[0] == "socket"):
                    # init's socket grammar has an optional SELinux context.
                    # Without one, it delegates to the service's verified
                    # process-transition target.
                    if len(socket) not in (6, 7):
                        raise ValueError("generic service socket syntax: " + name)
                    if any(item["name"] == socket[1] for item in sockets):
                        raise ValueError("duplicate generic service socket: " + name)
                    sockets.append({"name": socket[1], "type": socket[2],
                        "mode": socket[3], "uid": socket[4], "gid": socket[5],
                        "seclabel": socket[6] if len(socket) == 7 else None})
                services[name] = {"service": name, "rc": relative, "rc_line": section["line"],
                    "rc_sha256": _digest(data), "executable": expand(words[2]),
                    "user": option("user", "root"), "seclabel": option("seclabel", None),
                    "sockets": sockets, "flags": flags}
        for section in sections:
            if section["header"][0] != "import":
                continue
            raw = section["header"][1]
            guest = expand(raw)
            path = root / guest.lstrip("/")
            if not path.exists() and raw in ("/init.${ro.hardware}.rc",
                    "/vendor/etc/init/hw/init.${ro.hardware}.rc"):
                continue
            if path.is_dir():
                for child in sorted(path.glob("*.rc")):
                    read(child.relative_to(root).as_posix())
            else:
                read(guest.lstrip("/"))

    read("scarlet/boot/init.rc")
    for directory in sorted(root.glob("system/apex/*/etc")):
        for path in _versioned_apex_configs(directory):
            read(path.relative_to(root).as_posix(), True)
    if not 1 <= len(services) <= 256 or len(configs) > 256:
        raise ValueError("generic init graph bound")
    for service in services.values():
        service.pop("flags")
    return configs, [services[name] for name in sorted(services)]


def export_generic(policy, setools, policy_path, root, boot_manifest, partitions,
                   packages, aid_header, debugfs="debugfs"):
    """Export source-backed service launches, sockets and init label restoration."""
    import ctypes
    import ctypes.util
    import posixpath
    import tempfile
    from aosp_inventory import Image
    from art_inputs import SDK_SHA256, quote_debugfs, sha256, unpack_apex

    policy_path, root, boot_manifest, partitions, packages, aid_header = map(Path,
        (policy_path, root, boot_manifest, partitions, packages, aid_header))
    if sha256(policy_path) != SDK_POLICY_SHA256 or policy.version != SDK_POLICY_VERSION:
        raise ValueError("generic compiled policy source")
    boot_bytes = boot_manifest.read_bytes()
    metadata = json.loads(boot_bytes)
    if metadata.get("sdk_sha256") != SDK_SHA256 or metadata.get("result") != "PREPARED":
        raise ValueError("generic boot source")
    package_manifest = packages / "stock-packages.json"
    package_data = json.loads(package_manifest.read_text())
    if (package_data.get("sdk_sha256") != SDK_SHA256 or
            package_data.get("partitions") != GENERIC_PARTITIONS or
            package_data.get("result") != "PREPARED"):
        raise ValueError("generic package source")
    for name, digest in GENERIC_PARTITIONS.items():
        if sha256(partitions / (name + ".ext4")) != digest:
            raise ValueError("generic source partition: " + name)
    images = {name: Image(partitions / (name + ".ext4"), debugfs) for name in GENERIC_PARTITIONS}
    # Compare the pinned copy to the actual source image too.
    if _digest(images["vendor"].command("cat /etc/selinux/precompiled_sepolicy")) != SDK_POLICY_SHA256:
        raise ValueError("generic source image policy")
    configs, services = _generic_graph(root, metadata)
    passwd = images["system"].command("cat /system/etc/passwd")
    header = aid_header.read_bytes()
    uids = generic_uids(passwd, header)
    manifest = {"kind": GENERIC_MAGIC, "source_sha256": SDK_POLICY_SHA256,
        "policy_version": policy.version, "setools_version": setools.__version__,
        "sdk_sha256": SDK_SHA256, "partitions": dict(GENERIC_PARTITIONS),
        "boot_manifest_sha256": _digest(boot_bytes), "package_manifest_sha256": sha256(package_manifest),
        "aid_header_sha256": _digest(header), "passwd_sha256": _digest(passwd),
        "rc_files": configs, "services": [], "file_labels": {}, "exec_transitions": [], "transitions": [],
        "socket_labels": [],
        "scope": "Effective init launches, declared sockets and canonical filesystem label restoration; no file content writes or blanket service access"}
    nodes = package_data["nodes"]
    bindings = metadata["profile"].get("file_labels", {})
    existing = metadata["profile"].get("exec_transitions", [])
    # Do not read mutable host paths through guest absolute symlinks.
    def staged(name, depth=0):
        if depth > 8:
            raise ValueError("generic executable alias depth")
        record = nodes.get(name)
        if record and "target" in record:
            return staged(posixpath.normpath(posixpath.join("/", posixpath.dirname(name),
                                                          record["target"])).lstrip("/"), depth + 1)
        path = root / name
        if path.is_symlink():
            return staged(posixpath.normpath(posixpath.join("/", posixpath.dirname(name),
                                                          str(path.readlink()))).lstrip("/"), depth + 1)
        if path.is_file():
            digest = sha256(path)
            expected = metadata["files"].get(name) or (record or {}).get("sha256")
            if digest != expected:
                raise ValueError("changed staged generic executable: " + name)
            return name, digest
        if record and "sha256" in record:
            path = packages / "objects" / record["sha256"]
            if sha256(path) != record["sha256"]:
                raise ValueError("changed generic package executable: " + name)
            return name, record["sha256"]
        if name.startswith("apex/"):
            return staged("system/" + name, depth + 1)
        return name, None

    with tempfile.TemporaryDirectory(prefix="scarlet-generic-policy-") as temporary:
        temporary = Path(temporary)
        context_sources = {"system:/system/etc/selinux/plat_file_contexts":
            images["system"].command("cat /system/etc/selinux/plat_file_contexts"),
            "vendor:/etc/selinux/vendor_file_contexts":
            images["vendor"].command("cat /etc/selinux/vendor_file_contexts")}
        manifest["file_contexts"] = {name: _digest(data) for name, data in context_sources.items()}
        if metadata["profile"].get("file_policy", {}).get("default") == "u:object_r:unlabeled:s0":
            from boot_profile import file_rules
            configured_contexts, _, _ = file_rules(metadata["profile"])
            manifest["file_relabels"], manifest["file_relabel_evidence"] = generic_file_relabels(policy, setools, context_sources, configured_contexts)
            if "u:r:installd:s0" in metadata["profile"]["subjects"]:
                grants, evidence = generic_file_relabels(policy, setools, context_sources, configured_contexts, "installd")
                manifest["file_relabels"] = sorted(manifest["file_relabels"] + grants)
                manifest["file_relabel_evidence"] = sorted(set(manifest["file_relabel_evidence"] + evidence))
        context_path = temporary / "file_contexts"
        context_path.write_bytes(b"\n".join(context_sources.values()))
        library = ctypes.CDLL(ctypes.util.find_library("selinux"), use_errno=True)
        class Option(ctypes.Structure):
            _fields_ = [("type", ctypes.c_int), ("value", ctypes.c_char_p)]
        library.selabel_open.argtypes = [ctypes.c_uint, ctypes.POINTER(Option), ctypes.c_uint]
        library.selabel_open.restype = ctypes.c_void_p
        library.selabel_lookup_raw.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p), ctypes.c_char_p, ctypes.c_int]
        library.selabel_close.argtypes = [ctypes.c_void_p]
        library.freecon.argtypes = [ctypes.c_void_p]
        handle = library.selabel_open(0, (Option * 1)(Option(3, str(context_path).encode())), 1)
        if not handle:
            raise ValueError("generic source file contexts")
        try:
            for service in services:
                record = dict(service)
                user = service["user"]
                principal = int(user) if user.isdecimal() else uids.get(user)
                if principal is None or not 0 <= principal <= 0xffffffff:
                    raise ValueError("unknown generic service UID: " + user)
                record["principal"] = principal
                guest = service["executable"]
                resolved, actual_digest = staged(guest.lstrip("/"))
                record.update(resolved_executable="/" + resolved, executable_sha256=actual_digest)
                if actual_digest is None:
                    from stock_packages import OPTIONAL_SERVICE_EXECUTABLES
                    if (service["service"], guest) not in OPTIONAL_SERVICE_EXECUTABLES:
                        raise ValueError("missing generic executable: " + guest)
                    record["disposition"] = "optional-absent"
                    manifest["services"].append(record)
                    continue
                # Existing administrator bindings include native provider builds
                # and deliberate machine policy. Preserve them; never claim that
                # those records came from Android's compiled policy.
                managed = False
                if guest in bindings:
                    authorized = [rule for rule in existing if rule[0] == "u:r:init:s0" and
                                  rule[1] == bindings[guest] and rule[3] == principal]
                    if authorized:
                        record.update(disposition="profile-managed", file_label=bindings[guest],
                                      profile_exec_transitions=authorized)
                        managed = True
                    if service["seclabel"] in metadata["profile"]["subjects"]:
                        transition = ["u:r:init:s0", service["seclabel"], principal]
                        if transition in metadata["profile"]["transitions"]:
                            record.update(disposition="profile-managed", file_label=bindings[guest],
                                          profile_transition=transition)
                            managed = True
                # Native machine executables have no Android source inode.
                # They remain wholly under the pre-existing profile proof.
                if managed and not (resolved in nodes or resolved.startswith("system/apex/")):
                    record["context_validation"] = "existing-machine-profile"
                    manifest["services"].append(record)
                    continue
                image, source = None, None
                apex_proof = None
                if resolved.startswith("system/apex/"):
                    module, _, inside = resolved[len("system/apex/"):].partition("/")
                    candidates = [item for item in metadata["factory_apexes"] if item["name"] == module]
                    if len(candidates) != 1 or not inside:
                        raise ValueError("generic APEX source: " + guest)
                    apex_proof = candidates[0]
                    archive, payload = temporary / "module.zip", temporary / "payload.ext4"
                    for path in (archive, payload, payload.with_suffix(".apex")):
                        path.unlink(missing_ok=True)
                    images["system"].dump(apex_proof["source"], archive)
                    if sha256(archive) != apex_proof["package_sha256"]:
                        raise ValueError("generic APEX package hash")
                    unpack_apex(archive, payload)
                    if sha256(payload) != apex_proof["payload_sha256"]:
                        raise ValueError("generic APEX payload hash")
                    image, source = Image(payload, debugfs), "/" + inside
                    record["apex"] = apex_proof
                else:
                    partition, _, inside = resolved.partition("/")
                    if partition not in images:
                        raise ValueError("generic executable outside source images: " + guest)
                    image = images[partition]
                    source = "/" + resolved if partition == "system" else "/" + inside
                record["source_path"] = source
                data = image.command("cat " + quote_debugfs(source))
                record["source_executable_sha256"] = _digest(data)
                if _digest(data) != actual_digest and not managed:
                    raise ValueError("generic executable differs from source: " + guest)
                raw = image.command("ea_get " + quote_debugfs(source) + " security.selinux").decode()
                match = re.fullmatch(r'security.selinux \((\d+)\) = "(u:object_r:[A-Za-z0-9_]+:s0)\\000"\s*', raw)
                if not match or int(match[1]) != len(match[2]) + 1:
                    raise ValueError("missing generic source SELinux xattr: " + guest)
                label = match[2]
                record.update(source_file_label=label, source_xattr=raw,
                              source_xattr_sha256=_digest((label + "\0").encode()))
                if apex_proof is None:
                    result = ctypes.c_void_p()
                    if library.selabel_lookup_raw(handle, ctypes.byref(result), ("/" + resolved).encode(), 0o100000):
                        raise ValueError("generic file context lookup: " + guest)
                    try:
                        context = ctypes.string_at(result).decode()
                    finally:
                        library.freecon(result)
                    if context != label:
                        raise ValueError("generic xattr/file-context mismatch: " + guest)
                    record["context_validation"] = "stock-file-contexts"
                else:
                    # APEX file labels reside on the signed payload inode; the
                    # platform contexts have no matching per-module bin rules.
                    record["context_validation"] = "verified-apex-payload-xattr"
                if managed and service["seclabel"] is None and (
                        record["file_label"] != label or _digest(data) != actual_digest):
                    record["selection"] = "existing profile overrides stock executable label or bytes"
                    manifest["services"].append(record)
                    continue
                try:
                    target, evidence = generic_transition(policy, setools, label, service["seclabel"])
                except ValueError as error:
                    raise ValueError(f"generic service {service['service']} ({guest}): {error}") from error
                if managed:
                    record.update(source_target=target, policy_rules=evidence)
                    manifest["services"].append(record)
                    continue
                record["file_label"] = label
                record.update(disposition="source-derived", target=target, policy_rules=evidence)
                manifest["file_labels"]["/" + resolved] = label
                socket_labels = set()
                for socket in record["sockets"]:
                    socket_label = socket["seclabel"] or target
                    if not re.fullmatch(r"u:(r|object_r):[A-Za-z0-9_]+:s0", socket_label):
                        raise ValueError("generic service socket label: " + service["service"])
                    policy.lookup_type(socket_label.split(":")[2])
                    socket_labels.add(socket_label)
                if socket_labels:
                    record["socket_labels"] = sorted(socket_labels)
                    manifest["socket_labels"].extend(
                        ["u:r:init:s0", socket_label] for socket_label in sorted(socket_labels))
                # Explicit seclabel is a delegated setexeccon selection, not an
                # automatic inode transition (ueventd aliases init itself).
                field = "transitions" if service["seclabel"] else "exec_transitions"
                transition = (["u:r:init:s0", target, principal] if service["seclabel"] else
                              ["u:r:init:s0", label, target, principal])
                if transition not in manifest[field]:
                    manifest[field].append(transition)
                manifest["services"].append(record)
        finally:
            library.selabel_close(handle)
    # Catch concurrent source/profile changes before publishing output.
    if sha256(boot_manifest) != manifest["boot_manifest_sha256"] or sha256(policy_path) != SDK_POLICY_SHA256:
        raise ValueError("generic source changed during export")
    for name, digest in GENERIC_PARTITIONS.items():
        if sha256(partitions / (name + ".ext4")) != digest:
            raise ValueError("generic source partition changed during export: " + name)
    if sha256(package_manifest) != manifest["package_manifest_sha256"]:
        raise ValueError("generic package manifest changed during export")
    manifest["exec_transitions"].sort()
    manifest["transitions"].sort()
    manifest["socket_labels"] = [list(rule) for rule in sorted({
        tuple(rule) for rule in manifest["socket_labels"]})]
    manifest["manifest_sha256"] = _digest(json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode())
    validate_generic(manifest)
    return manifest


def validate_generic(manifest):
    """Validate a byte-bound exported selection before adding native records."""
    value = dict(manifest)
    digest = value.pop("manifest_sha256", None)
    if digest != _digest(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()):
        raise ValueError("changed generic exec manifest")
    if (value.get("kind") != GENERIC_MAGIC or value.get("source_sha256") != SDK_POLICY_SHA256 or
            value.get("policy_version") != SDK_POLICY_VERSION or value.get("partitions") != GENERIC_PARTITIONS or
            value.get("aid_header_sha256") != AID_HEADER_SHA256):
        raise ValueError("generic exec provenance")
    from boot_profile import relative_path, valid_context
    if not 1 <= len(value["services"]) <= 256 or len(value["rc_files"]) > 256:
        raise ValueError("generic manifest count")
    for name, digest in value["rc_files"].items():
        relative_path(name)
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ValueError("generic RC digest")
    files, transitions, direct, socket_rules, names = {}, set(), set(), set(), set()
    socket_count = 0
    for service in value["services"]:
        if (service["service"] in names or type(service["principal"]) is not int or
                not 0 <= service["principal"] <= 0xffffffff or
                value["rc_files"].get(service["rc"]) != service["rc_sha256"]):
            raise ValueError("generic service record")
        names.add(service["service"])
        sockets = service.get("sockets", [])
        if type(sockets) is not list or len(sockets) > 4096:
            raise ValueError("generic service sockets")
        socket_count += len(sockets)
        if socket_count > 4096:
            raise ValueError("generic service socket bound")
        socket_names = set()
        for socket in sockets:
            if (type(socket) is not dict or set(socket) != {"name", "type", "mode", "uid", "gid", "seclabel"} or
                    socket["name"] in socket_names or any(type(socket[key]) is not str or not socket[key]
                    for key in ("name", "type", "mode", "uid", "gid")) or
                    socket["seclabel"] is not None and (type(socket["seclabel"]) is not str or
                    not re.fullmatch(r"u:(r|object_r):[A-Za-z0-9_]+:s0", socket["seclabel"]))):
                raise ValueError("generic service socket record")
            socket_names.add(socket["name"])
        if service["disposition"] == "source-derived":
            path, label, target = service["resolved_executable"], service["file_label"], service["target"]
            relative_path(path.lstrip("/"))
            if (not path.startswith(("/system/", "/system_ext/", "/product/", "/vendor/", "/apex/")) or
                    not valid_context(label) or not label.startswith("u:object_r:") or
                    not re.fullmatch(r"u:r:[A-Za-z0-9_]+:s0", target) or
                    not re.fullmatch(r"[0-9a-f]{64}", service["executable_sha256"]) or
                    service["source_xattr_sha256"] != _digest((label + "\0").encode()) or not service["policy_rules"]):
                raise ValueError("generic source record")
            if path in files and files[path] != label:
                raise ValueError("generic file binding conflict")
            files[path] = label
            if service["seclabel"]:
                if service["seclabel"] != target:
                    raise ValueError("generic explicit target mismatch")
                direct.add(("u:r:init:s0", target, service["principal"]))
            else:
                transitions.add(("u:r:init:s0", label, target, service["principal"]))
            expected_sockets = sorted({socket["seclabel"] or target for socket in sockets})
            if service.get("socket_labels", []) != expected_sockets:
                raise ValueError("generic service socket delegation differs from options")
            socket_rules.update(("u:r:init:s0", socket_label) for socket_label in expected_sockets)
        elif service["disposition"] not in ("profile-managed", "optional-absent"):
            raise ValueError("generic disposition")
        elif service.get("socket_labels"):
            raise ValueError("generic managed service socket delegation")
    if (files != value["file_labels"] or sorted(transitions) != [tuple(rule) for rule in value["exec_transitions"]] or
            sorted(direct) != [tuple(rule) for rule in value["transitions"]] or
            value.get("socket_labels", []) != sorted(map(list, socket_rules))):
        raise ValueError("generic exec selection differs from evidence")
    relabels = value.get("file_relabels", [])
    evidence = value.get("file_relabel_evidence", [])
    if (not isinstance(relabels, list) or len(relabels) > 4096 or
            not isinstance(evidence, list) or len(evidence) > 4096 or
            bool(relabels) != bool(evidence) or
            any(not isinstance(line, str) or not re.match(r"allow (init|installd) ", line) or len(line) > 4096 for line in evidence)):
        raise ValueError("generic init relabel evidence")
    for rule in relabels:
        if (not isinstance(rule, list) or len(rule) != 4 or
                rule[0] not in ("u:r:init:s0", "u:r:installd:s0") or rule[1] != "u:object_r:unlabeled:s0" or
                not isinstance(rule[2], str) or not re.fullmatch(r"u:object_r:[A-Za-z0-9_]+:s0", rule[2]) or
                rule[2] == rule[1] or type(rule[3]) is not int or not 1 <= rule[3] <= 7):
            raise ValueError("generic init file relabel")
    if len({tuple(rule) for rule in relabels}) != len(relabels):
        raise ValueError("duplicate generic init file relabel")
    return manifest


def apply_generic_boot_policy(boot, manifest, root=None):
    """Append verified launch, socket and label records with existing identifiers.

    Apply before apply_boot_policy() so the ordinary source service export also
    selects the newly registered daemon subjects. File content writes and
    service access are separate; sockets follow selected service declarations.
    """
    validate_generic(manifest)
    if root is not None:
        from art_inputs import sha256
        root = Path(root)
        for name, digest in manifest["rc_files"].items():
            if sha256(root / name) != digest:
                raise ValueError("changed generic RC during apply: " + name)
    lines = boot.splitlines()
    if not lines or lines[0] != "scarlet-android-boot-policy-v2":
        raise ValueError("generic exec requires boot policy v2")
    labels, ids, files, rules, direct, sockets, relabels = {}, set(), {}, {}, set(), set(), set()
    file_default = None
    for line in lines:
        fields = line.split()
        if fields[0] == "label":
            identifier = int(fields[1])
            if fields[2] in labels or identifier in ids:
                raise ValueError("duplicate generic boot label")
            labels[fields[2]], ids = identifier, ids | {identifier}
        elif fields[0] == "file-label":
            files[fields[2]] = int(fields[1])
        elif fields[0] == "file-exec":
            key = (int(fields[1]), int(fields[2]), int(fields[4]))
            rules[key] = int(fields[3])
        elif fields[0] == "transition":
            direct.add(tuple(map(int, fields[1:])))
        elif fields[0] == "socket-create":
            sockets.add((int(fields[1]), int(fields[2])))
        elif fields[0] == "file-default":
            file_default = int(fields[1])
        elif fields[0] == "file-relabel":
            relabels.add(tuple(map(int, fields[1:])))
    added = []
    def identifier(label):
        if label not in labels:
            labels[label] = max(ids, default=0) + 1
            ids.add(labels[label])
            added.append(f"label {labels[label]} {label}")
        return labels[label]
    for path, label in sorted(manifest["file_labels"].items()):
        number = identifier(label)
        if path in files and files[path] != number:
            raise ValueError("generic binding conflicts with explicit profile: " + path)
        if path not in files:
            added.append(f"file-label {number} {path}")
            files[path] = number
    for source, resource, target, principal in manifest["exec_transitions"]:
        source, resource, target = map(identifier, (source, resource, target))
        key = (source, resource, principal)
        if key in rules and rules[key] != target:
            raise ValueError("generic transition conflicts with explicit profile")
        if key not in rules:
            added.append(f"file-exec {source} {resource} {target} {principal}")
            rules[key] = target
    for source, target, principal in manifest["transitions"]:
        rule = (identifier(source), identifier(target), principal)
        if rule not in direct:
            added.append("transition " + " ".join(map(str, rule)))
            direct.add(rule)
    for source, target in manifest.get("socket_labels", []):
        rule = (identifier(source), identifier(target))
        if rule not in sockets:
            added.append(f"socket-create {rule[0]} {rule[1]}")
            sockets.add(rule)
    if manifest.get("file_relabels") and file_default != labels.get("u:object_r:unlabeled:s0"):
        raise ValueError("init label restoration requires the matching native file default")
    for source, origin, target, kind in manifest.get("file_relabels", []):
        rule = (identifier(source), identifier(origin), identifier(target), kind)
        if rule not in relabels:
            added.append("file-relabel " + " ".join(map(str, rule)))
            relabels.add(rule)
    if len(labels) > MAX_NATIVE_LABELS or len(files) > 4096 or len(rules) > 16384 or len(direct) > 16384 or len(sockets) > 16384 or len(relabels) > 16384:
        raise ValueError("generic native policy bound")
    return "\n".join(lines + added) + "\n"
