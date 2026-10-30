#!/usr/bin/env python3
"""Execute a real CPU Vulkan buffer transfer in the private HVF fixture."""
import importlib.util
import json
from pathlib import Path
import shutil
import sys

from art_inputs import sha256

SOURCE=Path(__file__).resolve().parent


def main():
    arguments=sys.argv[1:]
    index=arguments.index("--software-vulkan")
    build=Path(arguments[index+1]).resolve();del arguments[index:index+2]
    record=json.loads((build/"build.json").read_text())
    if record.get("kind")!="android-software-vulkan-v1":raise ValueError("incomplete software Vulkan build")
    for path,digest in record["files"].items():
        if sha256(build/path)!=digest:raise ValueError("changed software Vulkan artifact")
    for path,digest in record["sources"].items():
        if sha256(SOURCE/path)!=digest:raise ValueError("stale software Vulkan source; rebuild")
    if any(flag in arguments for flag in ["--program","--program-env","--accel"]):raise ValueError("Vulkan runner selects its program and HVF accelerator")
    output=Path(arguments[arguments.index("--output")+1]).resolve()
    spec=importlib.util.spec_from_file_location("software_vulkan_direct",SOURCE/"run-direct.py")
    runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
    original=runner.stage_boot_inputs
    def stage(inputs,runtime,stock,root):
        metadata=original(inputs,runtime,stock,root)
        for path,digest in record["libraries"].items():
            if sha256(root/path)!=digest:raise ValueError("Vulkan platform library mismatch")
        target=root/"vendor/lib64/hw/vulkan.pastel.so"
        if target.exists() and sha256(target)!=record["files"]["vulkan.pastel.so"]:raise ValueError("conflicting Vulkan provider")
        target.parent.mkdir(exist_ok=True,parents=True);shutil.copy2(build/"vulkan.pastel.so",target)
        if sha256(target)!=record["files"]["vulkan.pastel.so"]:raise ValueError("changed Vulkan provider during staging")
        shutil.copy2(build/"build.json",output/"software-vulkan-build.json")
        return metadata
    runner.stage_boot_inputs=stage
    sys.argv=[str(SOURCE/"run-direct.py"),*arguments,"--program",str(build/"vulkan-probe"),"--init-bootstrap","--accel","hvf",
              "--program-env","LD_LIBRARY_PATH=/vendor/lib64:/system/lib64:/apex/com.android.runtime/lib64/bionic:/system/apex/com.android.i18n/lib64:/system/apex/com.android.art/lib64"]
    result=runner.main();serial=(output/"serial.log").read_bytes()
    passed=result==0 and b"SCARLET_ANDROID_VULKAN_TRANSFER_OK" in serial
    (output/"software-vulkan-result.json").write_text(json.dumps({"result":"PASS" if passed else "FAIL",
        "scope":"Android hardware-module entrypoint, CPU physical/logical device, command submission, fence wait and host readback",
        "surfaceflinger_composition":"not tested"},indent=2)+"\n")
    return 0 if passed else 1


if __name__=="__main__":raise SystemExit(main())
