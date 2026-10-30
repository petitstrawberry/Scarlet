#!/usr/bin/env python3
"""Observe stock init under an explicit manager profile, without asserting boot readiness."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys

from art_inputs import sha256

SOURCE = Path(__file__).resolve().parent


def main():
    arguments = sys.argv[1:]
    compositor=None
    if "--sws-native" in arguments:
        index=arguments.index("--sws-native");compositor=Path(arguments[index+1]).resolve();del arguments[index:index+2]
    start_zygote="--start-zygote" in arguments
    if start_zygote:arguments.remove("--start-zygote")
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
        if compositor:
            destination=root.parent.parent/"bin/sws";shutil.copy2(compositor,destination)
            if sha256(destination)!=sha256(compositor):raise ValueError("changed native SWS executable")
            (output/"native-sws.json").write_text(json.dumps({"path":str(compositor),"sha256":sha256(compositor),"backend":"cpu"},indent=2)+"\n")
        for partition in ["system","vendor"]:
            destination=root/partition/"lib64/libprobe-init-kmsg.so"
            destination.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(build/"libprobe-init-kmsg.so",destination)
            if sha256(destination)!=record["files"]["libprobe-init-kmsg.so"]:raise ValueError("log probe changed while staging")
        shutil.copy2(build / "build.json", output / "init-probe-build.json")
        (output / "init-diagnostic.json").write_text(json.dumps({
            "kind": "bounded-stock-init-profile-v1", "probe_build_sha256": sha256(build / "build.json"),
            "runner_sha256": sha256(SOURCE / "run-init.py"), "acceleration": "hvf",
            "observation": "20 seconds or init exit, then close ProcessScope and verify zero live members",
            "logging": "Diagnostic preload sends init/daemon messages to inherited console and Zygote messages through short-lived Unix datagrams; supervisor drains logdw wire records; runtime results remain unchanged",
            "service_readiness": "not asserted",
            "service_control_request": "ctl.start=zygote after 4 seconds" if start_zygote else None, "apex_activation": "stock apexd must perform activation",
        }, indent=2) + "\n")
        return metadata

    if start_zygote:arguments.extend(["--program-argument","start-zygote"])
    runner.stage_boot_inputs = stage
    original_run=runner.smoke.run_guest
    previous=os.environ.get("SCARLET_ANDROID_SWS_NATIVE")
    if compositor:
        os.environ["SCARLET_ANDROID_SWS_NATIVE"]="1"
        def run(command,directory,timeout):
            command=list(command)+["-device","virtio-gpu-device,bus=virtio-mmio-bus.3"]
            commands=json.loads((directory/"commands.json").read_text());commands["qemu"]=command
            (directory/"commands.json").write_text(json.dumps(commands,indent=2)+"\n")
            return original_run(command,directory,timeout)
        runner.smoke.run_guest=run
    sys.argv = [str(SOURCE / "run-direct.py"), *arguments, "--program", str(build / "init-scope-probe"), "--init-bootstrap", "--accel", "hvf"]
    try:return runner.main()
    finally:
        if compositor:
            if previous is None:os.environ.pop("SCARLET_ANDROID_SWS_NATIVE",None)
            else:os.environ["SCARLET_ANDROID_SWS_NATIVE"]=previous


if __name__ == "__main__":
    raise SystemExit(main())
