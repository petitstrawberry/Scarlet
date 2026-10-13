#!/usr/bin/env python3
"""Check complete support-probe records; boot identity needs a run manifest."""
import json
from pathlib import Path
import re
import sys

source = Path(__file__).with_name("native-support.c").read_text()
expected = re.findall(r'CHECK\("([^"]+)"', source)
if not expected or len(set(expected)) != len(expected):
    sys.exit("Probe source has no checks or duplicate check names")
if len(sys.argv) < 2:
    sys.exit("Usage: verify-native-support.py NATIVE_LOG [NATIVE_LOG ...]")

for name in sys.argv[1:]:
    text = Path(name).read_text()
    records = re.findall(r"WEBCORE_RUNTIME[^\r\n]*", text)
    found = []
    summaries = []
    for record in records:
        case = re.fullmatch(r"WEBCORE_RUNTIME (\w+) (PASS|FAIL)", record)
        if case:
            found.append(case.groups())
        elif record == "WEBCORE_RUNTIME RESULT failures=0":
            summaries.append(record)
        else:
            sys.exit(f"{name}: invalid or failing probe record: {record}")
    seen = {case for case, _ in found}
    if len(found) != len(expected) or seen != set(expected):
        sys.exit(f"{name}: missing, duplicate or unexpected checks: "
                 f"count={len(found)}, missing={set(expected) - seen}, extra={seen - set(expected)}")
    if any(status != "PASS" for _, status in found):
        sys.exit(f"{name}: failed checks")
    if len(summaries) != 1 or not records or records[-1] != summaries[0]:
        sys.exit(f"{name}: missing, duplicate or misplaced success result")
    print(json.dumps({"log": name, "checks": len(found), "failures": 0,
                      "marker": summaries[0]}))
