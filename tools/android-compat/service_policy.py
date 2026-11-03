"""Translate compiled Android service permissions, without interpreting CIL."""
import hashlib
import json
import re
from pathlib import Path

MAGIC = "scarlet-android-service-policy-v1"
CLASSES = ("service_manager", "hwservice_manager")
PERMISSIONS = {"add": 1, "find": 2, "list": 4}
# /vendor/etc/selinux/precompiled_sepolicy in the SDK pinned by art_inputs.py.
SDK_POLICY_SHA256 = "2be08e2a2e93d22c6a1c5e4e32fa4d571ecb07695877cdcfe0c3a9f48ba9234c"


def export(policy, setools):
    # A type-only Shield rule cannot represent conditional or MLS constraints.
    # Reject those inputs instead of silently granting a superset of access.
    if list(setools.ConstraintQuery(policy, tclass=CLASSES).results()):
        raise ValueError("service class constraints require a different translation")
    classes = {str(c): c for c in policy.classes() if str(c) in CLASSES}
    if set(classes) != set(CLASSES) or any(set(c.perms) != set(PERMISSIONS) for c in classes.values()):
        raise ValueError("unsupported Android service permission vocabulary")
    domains = {str(t) for t in policy.lookup_typeattr("domain").expand()}
    apps = {str(t) for t in policy.lookup_typeattr("appdomain").expand()}
    if not apps or not apps <= domains:
        raise ValueError("invalid app domain inventory")
    rules = {}
    target_classes = {}
    for rule in setools.TERuleQuery(policy, ruletype=["allow"], tclass=CLASSES).results():
        try:
            rule.conditional
        except setools.exception.RuleNotConditional:
            pass
        else:
            raise ValueError("conditional service rules require a different translation")
        sources = {str(t) for t in rule.source.expand()} & apps
        if not sources:
            continue
        if not set(rule.perms) <= set(PERMISSIONS):
            raise ValueError("unsupported service permission")
        mask = sum(PERMISSIONS[p] for p in rule.perms)
        for target in rule.target.expand():
            name = str(target)
            # The current Android ABI maps both classes to Shield's service
            # class. Only accept disjoint target types, so the mapping is exact.
            previous = target_classes.setdefault(name, str(rule.tclass))
            if previous != str(rule.tclass):
                raise ValueError("service classes overlap on target " + name)
            label = f"u:{'r' if name in domains else 'object_r'}:{name}:s0"
            for source in sources:
                key = (source, label)
                rules[key] = rules.get(key, 0) | mask
    lines = [MAGIC] + [f"domain {name}" for name in sorted(apps)]
    lines += [f"allow {source} {target} {mask}" for (source, target), mask in sorted(rules.items())]
    text = "\n".join(lines) + "\n"
    validate(text)
    return text


def validate(text):
    if len(text.encode()) > 1024 * 1024 or not text.endswith("\n"):
        raise ValueError("service policy size or termination")
    lines = text.splitlines()
    if not lines or lines[0] != MAGIC:
        raise ValueError("service policy version")
    domains, rules = set(), {}
    for line in lines[1:]:
        fields = line.split()
        if len(fields) == 2 and fields[0] == "domain":
            name = fields[1]
            if not re.fullmatch(r"[A-Za-z0-9_]+", name) or name in domains:
                raise ValueError("service policy domain")
            domains.add(name)
        elif len(fields) == 4 and fields[0] == "allow":
            _, source, target, permissions = fields
            if (source not in domains or
                    not re.fullmatch(r"u:(r|object_r):[A-Za-z0-9_]+:s0", target) or
                    permissions not in {"1", "2", "3", "4", "5", "6", "7"} or
                    (source, target) in rules):
                raise ValueError("service policy rule")
            rules[source, target] = int(permissions)
        else:
            raise ValueError("service policy record")
    if not domains or len(domains) > 256 or len(rules) > 16384:
        raise ValueError("service policy count")
    return domains, rules


def verify(directory):
    directory = Path(directory)
    metadata = json.loads((directory / "service-policy.json").read_text())
    path = directory / "service-policy.txt"
    if (metadata.get("kind") != MAGIC or
            metadata.get("source_sha256") != SDK_POLICY_SHA256 or
            metadata.get("text_sha256") != hashlib.sha256(path.read_bytes()).hexdigest()):
        raise ValueError("changed service policy output")
    validate(path.read_text())
    return metadata
