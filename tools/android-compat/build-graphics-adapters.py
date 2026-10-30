#!/usr/bin/env python3
"""Build the stock gralloc0-to-HIDL adapters against pinned Android inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import urllib.request

from art_inputs import sha256, elf_needed
from boot_profile import verify_boot_inputs
from libbinder_inputs import extract_headers

SOURCE=Path(__file__).resolve().parent
LIBRARIES=["libc++","libutils","libhidlbase","libhardware","liblog","libcutils","libsync",
           "android.hardware.graphics.mapper@2.0","android.hardware.graphics.allocator@2.0"]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ["ndk","runtime-root","stock-inputs","boot-profile","output"]:
        parser.add_argument("--"+name,type=Path,required=True)
    parser.add_argument("--reuse-headers",type=Path,help="reuse complete pinned trees instead of downloading")
    args=parser.parse_args()
    verify_boot_inputs(args.boot_profile,args.runtime_root,args.stock_inputs)
    output=args.output.resolve();output.mkdir(exist_ok=False,parents=True)
    pins=json.loads((SOURCE/"graphics/adapter-headers.json").read_text())
    for name,pin in pins["archives"].items():
        target=output/"headers"/name
        if args.reuse_headers:
            cache=args.reuse_headers/name
            if name=="vndk33" and not cache.exists():cache=args.reuse_headers/"vndk33-headers"
            paths=list(cache.rglob("*"))
            if cache.is_symlink() or any(p.is_symlink() for p in paths):raise ValueError("header tree contains aliases")
            contents={str(p.relative_to(cache)):sha256(p) for p in paths if p.is_file()}
            digest=hashlib.sha256(json.dumps(contents,sort_keys=True,separators=(",",":")).encode()).hexdigest()
            if digest!=pin["tree_sha256"]:raise ValueError("header tree differs from pinned contents")
            shutil.copytree(cache,target)
        else:
            url=f"https://android.googlesource.com/{pin['repository']}/+archive/{pin['revision']}/{pin['path']}.tar.gz"
            with urllib.request.urlopen(url,timeout=30) as response:data=response.read(8*1024**2+1)
            if len(data)>8*1024**2:raise ValueError("header download exceeds bound")
            extract_headers(data,pin["tree_sha256"],target,max_total=64*1024**2,max_files=10000)
    host={"Darwin":"darwin-x86_64","Linux":"linux-x86_64"}[platform.system()]
    compiler=args.ndk.resolve()/"toolchains/llvm/prebuilt"/host/"bin/aarch64-linux-android33-clang++"
    flags=[str(compiler),"-std=c++17","-O2","-fPIC","-fno-rtti","-fno-exceptions","-nostdinc++","-nostdlib++","-Wl,-z,defs"]
    vndk=output/"headers/vndk33"
    includes=["external/libcxx/include","system/libhidl/base/include","system/libhidl/transport/include",
              "system/libhwbinder/include","system/libfmq/base","system/core/libutils/include",
              "system/core/libcutils/include","system/core/libcutils/include_outside_system",
              "system/core/libsystem/include","system/libbase/include","system/logging/liblog/include_vndk",
              "hardware/libhardware/include","system/core/libsync/include"]
    for path in includes:flags += ["-isystem",str(vndk/path)]
    # Full qualified generated HIDL include names keep interface versions distinct.
    for path in sorted((vndk/"generated-headers").rglob("gen")):flags += ["-isystem",str(path)]
    for name in ["mapper-utils","allocator-utils"]:
        for kind in ["hal","passthrough"]:flags += ["-isystem",str(output/"headers"/name/kind/"include")]
    libraries=[args.boot_profile.resolve()/"system/lib64"/(name+".so") for name in LIBRARIES]
    link=[str(path) for path in libraries]
    products={"android.hardware.graphics.mapper@2.0-impl.scarlet.so":("mapper.cpp",["-shared"]),
              "android.hardware.graphics.allocator@2.0-service.scarlet":("allocator-service.cpp",["-pie"]),
              "graphics-adapter-probe":("adapter-probe.cpp",["-pie"])}
    commands=[]
    for name,(source,extra) in products.items():
        command=flags+extra+[str(SOURCE/"graphics"/source),*link,"-o",str(output/name)]
        subprocess.run(command,check=True);commands.append(command)
    record={"kind":"android-stock-graphics-adapters-v1","headers":pins,"commands":commands,
            "sources":{str(p.relative_to(SOURCE)):sha256(p) for p in [SOURCE/"build-graphics-adapters.py",SOURCE/"graphics/adapter-headers.json",*[SOURCE/"graphics"/entry[0] for entry in products.values()]]},
            "libraries":{str(path.relative_to(args.boot_profile.resolve())):sha256(path) for path in libraries},
            "compiler":subprocess.check_output([str(compiler),"--version"],text=True),
            "files":{name:sha256(output/name) for name in products},
            "needed":{name:elf_needed(output/name) for name in products},"readiness":"not executed"}
    (output/"build.json").write_text(json.dumps(record,indent=2)+"\n")
    print("Built stock graphics adapters; daemon registration and composition are not asserted")


if __name__=="__main__":main()
