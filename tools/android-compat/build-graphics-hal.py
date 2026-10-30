#!/usr/bin/env python3
"""Build the CPU gralloc/SWS framebuffer HAL against pinned Android headers."""
import argparse
import base64
import json
from pathlib import Path
import platform
import subprocess
import urllib.request

from art_inputs import elf_needed, sha256
from libbinder_inputs import prepare_headers

SOURCE = Path(__file__).resolve().parent


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--ndk",type=Path,required=True)
    p.add_argument("--output",type=Path,required=True)
    args=p.parse_args()
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    pins=json.loads((SOURCE/"graphics/headers.json").read_text())
    prepare_headers(output/"headers",pins)
    for name,pin in pins["files"].items():
        url=f"https://android.googlesource.com/{pin['repository']}/+/{pin['revision']}/{pin['path']}?format=TEXT"
        with urllib.request.urlopen(url,timeout=30) as response:
            raw=response.read(2*1024*1024+1)
        if len(raw)>2*1024*1024:raise ValueError("HAL header exceeds input bound")
        data=base64.b64decode(raw,validate=True)
        target=output/"headers/hardware"/name;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(data)
        if sha256(target)!=pin["sha256"]:raise ValueError("HAL header hash mismatch")
    host={"Darwin":"darwin-x86_64","Linux":"linux-x86_64"}[platform.system()]
    compiler=args.ndk.resolve()/"toolchains/llvm/prebuilt"/host/"bin/aarch64-linux-android33-clang"
    flags=[str(compiler),"-O2","-Wall","-Wextra","-Werror"]
    for name in ["hardware",*pins["archives"]]:flags += ["-isystem",str(output/"headers"/name)]
    flags += ["-I",str(SOURCE.parent.parent/"user/lib/sws-client-c/include")]
    hal=SOURCE/"graphics/gralloc.c"
    commands=[flags+["-shared","-fPIC","-Wl,-z,defs",str(hal),"-ldl","-o",str(output/"gralloc.scarlet.so")],
              flags+["-static",str(hal),str(SOURCE/"graphics/gralloc-probe.c"),"-ldl","-o",str(output/"gralloc-probe")]]
    for command in commands:subprocess.run(command,check=True)
    record={"kind":"android-cpu-graphics-hal-v1","commands":commands,"headers":pins,
            "sources":{str(name):sha256(SOURCE/name) for name in ["build-graphics-hal.py","graphics/headers.json","graphics/gralloc.c","graphics/gralloc-probe.c"]},
            "compiler":subprocess.check_output([str(compiler),"--version"],text=True),
            "needed":elf_needed(output/"gralloc.scarlet.so"),
            "files":{name:sha256(output/name) for name in ["gralloc.scarlet.so","gralloc-probe"]},
            "readiness":"not executed"}
    (output/"build.json").write_text(json.dumps(record,indent=2)+"\n")
    print("Built CPU buffer/framebuffer HAL; execution and stock-adapter integration are not asserted")


if __name__=="__main__":main()
