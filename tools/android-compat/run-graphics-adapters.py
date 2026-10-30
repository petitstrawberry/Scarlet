#!/usr/bin/env python3
"""Exercise stock HIDL Mapper/Allocator templates over the CPU gralloc HAL."""
import importlib.util
import json
from pathlib import Path
import shutil
import sys

from art_inputs import sha256

SOURCE=Path(__file__).resolve().parent


def main():
    arguments=sys.argv[1:];selected={};builds={}
    for name,kind in [("graphics-hal","android-cpu-graphics-hal-v1"),("graphics-adapters","android-stock-graphics-adapters-v1")]:
        flag="--"+name
        if flag not in arguments:raise ValueError("required: "+flag)
        index=arguments.index(flag);selected[name]=Path(arguments[index+1]).resolve();del arguments[index:index+2]
        record=json.loads((selected[name]/"build.json").read_text())
        if record.get("kind")!=kind:raise ValueError("unexpected graphics build kind")
        for path,digest in record["files"].items():
            if sha256(selected[name]/path)!=digest:raise ValueError("changed graphics build artifact")
        for path,digest in record["sources"].items():
            if sha256(SOURCE/path)!=digest:raise ValueError("stale graphics source; rebuild")
        builds[name]=record
    if any(flag in arguments for flag in ["--program","--accel","--program-env","--program-argument"]):
        raise ValueError("adapter runner selects its program, environment and HVF accelerator")
    output=Path(arguments[arguments.index("--output")+1]).resolve()
    spec=importlib.util.spec_from_file_location("graphics_adapter_direct",SOURCE/"run-direct.py")
    runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
    original_stage=runner.stage_boot_inputs
    def stage(inputs,runtime,stock,android):
        record=original_stage(inputs,runtime,stock,android)
        for path,digest in builds["graphics-adapters"]["libraries"].items():
            if sha256(android/path)!=digest:raise ValueError("graphics adapters linked against different platform libraries")
        products=[(selected["graphics-hal"]/"gralloc.scarlet.so","vendor/lib64/hw/gralloc.scarlet.so"),
                  (selected["graphics-hal"]/"gralloc.scarlet.so","vendor/lib64/hw/gralloc.default.so"),
                  (selected["graphics-adapters"]/"android.hardware.graphics.mapper@2.0-impl.scarlet.so",
                   "vendor/lib64/hw/android.hardware.graphics.mapper@2.0-impl.scarlet.so")]
        # This standalone fixture has no Android property service; libhardware
        # therefore selects its real default provider. Stock init uses scarlet.
        for original,name in products:
            destination=android/name
            if destination.exists():raise ValueError("graphics overlay conflicts with stock input")
            destination.parent.mkdir(exist_ok=True,parents=True);shutil.copy2(original,destination)
            if sha256(original)!=sha256(destination):raise ValueError("changed graphics overlay")
        for name in builds:shutil.copy2(selected[name]/"build.json",output/(name+"-build.json"))
        return record
    runner.stage_boot_inputs=stage
    sys.argv=[str(SOURCE/"run-direct.py"),*arguments,"--program",str(selected["graphics-adapters"]/"graphics-adapter-probe"),
              "--program-env","LD_LIBRARY_PATH=/vendor/lib64/hw:/system/lib64:/apex/com.android.runtime/lib64/bionic", "--init-bootstrap","--accel","hvf"]
    result=runner.main()
    serial=(output/"serial.log").read_bytes()
    passed=result==0 and b"SCARLET_ANDROID_GRAPHICS_ADAPTERS_OK" in serial
    (output/"graphics-adapters-result.json").write_text(json.dumps({"result":"PASS" if passed else "FAIL",
        "scope":"stock in-process Mapper/Allocator templates and imported CPU buffer lifetime",
        "allocator_hwbinder_registration":"not tested","surfaceflinger":"not tested"},indent=2)+"\n")
    print("Stock graphics adapter templates:","PASS" if passed else "FAIL")
    return 0 if passed else 1


if __name__=="__main__":raise SystemExit(main())
