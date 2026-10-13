#!/usr/bin/env python3
"""Validate complete native integer-rounding probe evidence from a guest log."""
import argparse
from collections import Counter
from pathlib import Path
import re
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    args = parser.parse_args()
    source = Path(__file__).with_name("math-round.c").read_text()
    names = re.findall(r'\{\s*"([a-z_]+)"\s*,', source)
    counts = Counter(names)
    if len(counts) != 44 or set(counts.values()) != {2}:
        sys.exit("Probe source does not contain the expected 44 cases per format")
    expected = {f"{function}_{name}" for function in
                ("lround", "llround", "lroundf", "llroundf") for name in counts}
    log = args.log.read_text(errors="replace")
    # Logs can include a boot/serial prefix, but the probe record is exact.
    records = re.findall(r"WEBCORE_MATH_ROUND ([a-z_]+) (PASS|FAIL)\r?$", log, re.M)
    seen = Counter(name for name, _ in records)
    problems = []
    if set(seen) != expected:
        problems.append(f"missing={sorted(expected - set(seen))}, extra={sorted(set(seen) - expected)}")
    if any(count != 1 for count in seen.values()):
        problems.append("duplicate check records")
    if any(status != "PASS" for _, status in records):
        problems.append("failed checks")
    summaries = re.findall(r"WEBCORE_MATH_ROUND RESULT checks=(\d+) calls=(\d+) failures=(\d+)\r?$", log, re.M)
    if summaries != [("176", "5632", "0")]:
        problems.append(f"incorrect or missing final result: {summaries}")
    if "WEBCORE_MATH_ROUND DETAIL " in log:
        problems.append("diagnostic failure records")
    if problems:
        sys.exit("Invalid native math probe log: " + "; ".join(problems))
    print("Native math rounding: all 176 checks / 5632 actual calls passed")


if __name__ == "__main__":
    main()
