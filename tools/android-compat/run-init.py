#!/usr/bin/env python3
"""Observe stock init under an explicit manager profile, without asserting boot readiness."""
import importlib.util
import json
from pathlib import Path
import shutil
import sys

from art_inputs import sha256

SOURCE = Path(__file__).resolve().parent


def main():
    arguments = sys.argv[1:]
    if "--init-probe" not in arguments or "--boot-profile" not in arguments:
        raise ValueError("run-init requires --init-probe BUILD and --boot-profile INPUTS")
    index = arguments.index("--init-probe")
    build = Path(arguments[index + 1]).resolve()
    del arguments[index:index + 2]
    record = json.loads((build / "build.json").read_text())
    if record.get("kind") != "bounded-stock-init-probe-v1" or record.get("result") != "BUILT":
        raise ValueError("incomplete init probe build")
    for name in ("init-scope-probe.c", "init-log-probe.c"):
        if record["sources_sha256"].get(name) != sha256(SOURCE / name):
            raise ValueError("stale init probe source")
    for name in ("init-scope-probe", "libprobe-init-kmsg.so"):
        if record["files"].get(name) != sha256(build / name):
            raise ValueError("changed init probe binary")
    if any(flag in arguments for flag in ["--program", "--init-bootstrap", "--accel"]):
        raise ValueError("run-init selects its bounded program and HVF accelerator")
    output = Path(arguments[arguments.index("--output") + 1])
    spec = importlib.util.spec_from_file_location("direct_init_runner", SOURCE / "run-direct.py")
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    original = runner.stage_boot_inputs

    def stage(inputs, runtime, stock, root):
        metadata = original(inputs, runtime, stock, root)
        shutil.copy2(build / "libprobe-init-kmsg.so", root / "system/lib64/libprobe-init-kmsg.so")
        if sha256(root / "system/lib64/libprobe-init-kmsg.so") != record["files"]["libprobe-init-kmsg.so"]:
            raise ValueError("log probe changed while staging")
        shutil.copy2(build / "build.json", output / "init-probe-build.json")
        (output / "init-diagnostic.json").write_text(json.dumps({
            "kind": "bounded-stock-init-profile-v1", "probe_build_sha256": sha256(build / "build.json"),
            "runner_sha256": sha256(SOURCE / "run-init.py"), "acceleration": "hvf",
            "observation": "20 seconds or init exit, then close ProcessScope and verify zero live members",
            "logging": "Diagnostic preload redirects userspace /dev/kmsg writes to inherited console; no syscall-success bypass",
            "service_readiness": "not asserted", "apex_activation": "stock apexd must perform activation",
        }, indent=2) + "\n")
        return metadata

    runner.stage_boot_inputs = stage
    sys.argv = [str(SOURCE / "run-direct.py"), *arguments, "--program", str(build / "init-scope-probe"), "--init-bootstrap", "--accel", "hvf"]
    return runner.main()


if __name__ == "__main__":
    raise SystemExit(main())
