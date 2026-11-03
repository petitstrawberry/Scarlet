"""Translate compiled Android service permissions, without interpreting CIL."""
import hashlib
import json
import re
from pathlib import Path

MAGIC = "scarlet-android-service-policy-v1"
ACCESS_MAGIC = "scarlet-android-access-policy-v2"
CLASSES = ("service_manager", "hwservice_manager")
PERMISSIONS = {"add": 1, "find": 2, "list": 4}
# /vendor/etc/selinux/precompiled_sepolicy in the SDK pinned by art_inputs.py.
SDK_POLICY_SHA256 = "2be08e2a2e93d22c6a1c5e4e32fa4d571ecb07695877cdcfe0c3a9f48ba9234c"


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
    if (metadata.get("kind") not in (MAGIC, ACCESS_MAGIC) or
            path.read_text().splitlines()[:1] != [metadata["kind"]] or
            metadata.get("source_sha256") != SDK_POLICY_SHA256 or
            metadata.get("text_sha256") != hashlib.sha256(path.read_bytes()).hexdigest()):
        raise ValueError("changed service policy output")
    validate(path.read_text())
    return metadata


def apply_boot_policy(boot, service_text):
    """Use the same compiled rules for existing boot subjects and future apps."""
    domains, grants = validate(service_text)
    labels, identifiers = {}, set()
    lines = boot.splitlines()
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
    sources = {name.split(":")[2]: identifier for name, identifier in labels.items()
               if re.fullmatch(r"u:r:[A-Za-z0-9_]+:s0", name) and name.split(":")[2] in domains}
    replaced = set(sources.values())
    replaced_kinds = ("service-allow ", "property-set ") if service_text.startswith(ACCESS_MAGIC + "\n") else ("service-allow ",)
    kept = [line for line in lines if not (line.startswith(replaced_kinds) and int(line.split()[1]) in replaced)]
    next_id = max(identifiers, default=0) + 1
    added, rules = [], []
    for (source, target, kind), mask in sorted(grants.items()):
        if source not in sources:
            continue
        if target not in labels:
            labels[target] = next_id
            added.append(f"label {next_id} {target}")
            next_id += 1
        rules.append(f"service-allow {sources[source]} {labels[target]} {mask}" if kind == 1 else f"property-set {sources[source]} {labels[target]}")
    if len(labels) > 1024 or sum(line.startswith(("service-allow ", "property-set ", "socket-create ", "keystore-allow ")) for line in kept) + len(rules) > 16384:
        raise ValueError("compiled service policy exceeds native policy bounds")
    return "\n".join(kept + added + rules) + "\n"
