#!/usr/bin/env python3
"""Export app service permissions from an Android compiled policy using SETools."""
import argparse
import hashlib
import json
from pathlib import Path

from service_policy import export, validate, validate_metadata


def main():
    import setools
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--domain", action="append", default=[],
                        help="also export a daemon domain used by the hosted device")
    parser.add_argument("--properties", action="store_true", help="also translate stock property_service set grants")
    args = parser.parse_args()
    data = args.policy.read_bytes()
    policy = setools.SELinuxPolicy(str(args.policy))
    text = export(policy, setools, args.domain, properties=args.properties)
    if args.policy.read_bytes() != data:
        raise ValueError("compiled policy changed during export")
    domains, rules = validate(text)
    metadata = {"kind": text.splitlines()[0], "source_sha256": hashlib.sha256(data).hexdigest(),
                "source": str(args.policy), "setools_version": setools.__version__,
                "policy_version": policy.version, "domains": len(domains),
                "rules": len(rules), "text_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "additional_domains": sorted(set(args.domain)),
                "properties": args.properties,
                "scope": "App and explicitly requested daemon service allows, plus property_service set when requested; not a full SELinux implementation"}
    validate_metadata(metadata, text, data)
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "service-policy.txt").write_text(text)
    (args.output / "service-policy.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Exported {len(rules)} service rules for {len(domains)} app domains")


if __name__ == "__main__":
    main()
