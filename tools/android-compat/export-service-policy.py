#!/usr/bin/env python3
"""Export app service permissions from an Android compiled policy using SETools."""
import argparse
import hashlib
import json
from pathlib import Path

from service_policy import export, export_generic, validate, validate_metadata


def main():
    import setools
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--domain", action="append", default=[],
                        help="also export a daemon domain used by the hosted device")
    parser.add_argument("--properties", action="store_true", help="also translate stock property_service set grants")
    parser.add_argument("--generic-root", type=Path, help="assembled Android view; export its effective init executable transitions")
    parser.add_argument("--generic-boot-profile", type=Path, help="prepared boot-profile.json including stock_init assembly hashes")
    parser.add_argument("--generic-partitions", type=Path, help="hash-pinned SDK ext4 partition directory")
    parser.add_argument("--generic-packages", type=Path, help="verified stock package object directory")
    parser.add_argument("--generic-aid-header", type=Path, help="pinned AOSP android_filesystem_config.h for built-in users")
    parser.add_argument("--debugfs", default="debugfs")
    args = parser.parse_args()
    generic_inputs = (args.generic_root, args.generic_boot_profile, args.generic_partitions,
                      args.generic_packages, args.generic_aid_header)
    if any(generic_inputs) and not all(generic_inputs):
        parser.error("generic export requires all five --generic-* inputs")
    data = args.policy.read_bytes()
    policy = setools.SELinuxPolicy(str(args.policy))
    generic = export_generic(policy, setools, args.policy, *generic_inputs,
                             debugfs=args.debugfs) if all(generic_inputs) else None
    additional = set(args.domain)
    if generic:
        additional.update(rule[2].split(":")[2] for rule in generic["exec_transitions"])
        additional.update(rule[1].split(":")[2] for rule in generic["transitions"])
        stock_domains = {str(t) for t in policy.lookup_typeattr("domain").expand()}
        profile = json.loads(args.generic_boot_profile.read_text())["profile"]
        additional.update(label.split(":")[2] for label in profile["subjects"]
                          if label.split(":")[2] in stock_domains)
    text = export(policy, setools, additional, properties=args.properties)
    if args.policy.read_bytes() != data:
        raise ValueError("compiled policy changed during export")
    domains, rules = validate(text)
    metadata = {"kind": text.splitlines()[0], "source_sha256": hashlib.sha256(data).hexdigest(),
                "source": str(args.policy), "setools_version": setools.__version__,
                "policy_version": policy.version, "domains": len(domains),
                "rules": len(rules), "text_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "additional_domains": sorted(additional),
                "properties": args.properties,
                "scope": "App and explicitly requested daemon service allows, plus property_service set when requested; not a full SELinux implementation"}
    validate_metadata(metadata, text, data)
    if generic:
        metadata["generic_exec_manifest_sha256"] = generic["manifest_sha256"]
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "service-policy.txt").write_text(text)
    (args.output / "service-policy.json").write_text(json.dumps(metadata, indent=2) + "\n")
    if generic:
        (args.output / "generic-service-policy.json").write_text(json.dumps(generic, indent=2) + "\n")
    print(f"Exported {len(rules)} service rules for {len(domains)} app domains")


if __name__ == "__main__":
    main()
