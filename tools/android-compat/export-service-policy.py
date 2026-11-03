#!/usr/bin/env python3
"""Export app service permissions from an Android compiled policy using SETools."""
import argparse
import hashlib
import json
from pathlib import Path

from service_policy import MAGIC, export, validate


def main():
    import setools
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    data = args.policy.read_bytes()
    policy = setools.SELinuxPolicy(str(args.policy))
    text = export(policy, setools)
    if args.policy.read_bytes() != data:
        raise ValueError("compiled policy changed during export")
    domains, rules = validate(text)
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "service-policy.txt").write_text(text)
    metadata = {"kind": MAGIC, "source_sha256": hashlib.sha256(data).hexdigest(),
                "source": str(args.policy), "setools_version": setools.__version__,
                "policy_version": policy.version, "domains": len(domains),
                "rules": len(rules), "text_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "scope": "App-domain service_manager and hwservice_manager allows only; not a full SELinux implementation"}
    (args.output / "service-policy.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Exported {len(rules)} service rules for {len(domains)} app domains")


if __name__ == "__main__":
    main()
