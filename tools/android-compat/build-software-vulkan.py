#!/usr/bin/env python3
"""Build pinned SwiftShader for Android's native-buffer/gralloc0 HAL path."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import urllib.request

from art_inputs import elf_needed,sha256
from boot_profile import verify_boot_inputs

SOURCE=Path(__file__).resolve().parent


def tree(directory):
    paths=list(directory.rglob("*"))
    if directory.is_symlink() or any(p.is_symlink() for p in paths):raise ValueError("source tree contains aliases")
    files={str(p.relative_to(directory)):sha256(p) for p in paths if p.is_file()}
    return hashlib.sha256(json.dumps(files,sort_keys=True,separators=(",",":")).encode()).hexdigest(),files


def write(path,data):
    path.parent.mkdir(exist_ok=True,parents=True)
    if not path.exists() or path.read_bytes()!=data:path.write_bytes(data)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ["ndk","source-cache","platform-headers","runtime-root","stock-inputs","boot-profile","output"]:
        parser.add_argument("--"+name,type=Path,required=True)
    parser.add_argument("--resume",action="store_true",help="rebuild an existing owned output using verified inputs")
    parser.add_argument("--jobs",type=int,choices=range(1,9),default=2)
    args=parser.parse_args()
    verify_boot_inputs(args.boot_profile,args.runtime_root,args.stock_inputs)
    pins=json.loads((SOURCE/"graphics/swiftshader-inputs.json").read_text())
    header_pin=json.loads((SOURCE/"graphics/adapter-headers.json").read_text())["archives"]["vndk33"]
    if tree(args.platform_headers)[0]!=header_pin["tree_sha256"]:raise ValueError("platform header tree changed")
    originals={}
    for name,pin in pins["archives"].items():
        digest,files=tree(args.source_cache/name)
        if digest!=pin["tree_sha256"]:raise ValueError("SwiftShader source tree differs from pinned contents: "+name)
        originals.update({name+"/"+path:args.source_cache/name/path for path in files})
    for name,digest in pins["files"].items():
        path=args.source_cache/name
        if sha256(path)!=digest:raise ValueError("SwiftShader root input changed")
        originals[name]=path
    output=args.output.resolve();output.mkdir(exist_ok=args.resume,parents=True)
    source=output/"source"
    for name,path in originals.items():write(source/name,path.read_bytes())
    for name,pin in pins["private_headers"].items():
        target=output/"private"/name
        if target.exists() and sha256(target)==pin["sha256"]:continue
        import base64
        with urllib.request.urlopen(pin["url"],timeout=30) as response:data=base64.b64decode(response.read(1024**2),validate=True)
        if hashlib.sha256(data).hexdigest()!=pin["sha256"]:raise ValueError("Android Vulkan header hash mismatch")
        write(target,data)
    headers=args.platform_headers.resolve()
    includes=[output/"private",*[headers/path for path in ["hardware/libhardware/include","system/core/libcutils/include",
        "system/core/libsync/include","frameworks/native/libs/nativebase/include","frameworks/native/libs/nativewindow/include","system/core/libsystem/include"]]]
    libraries=[args.boot_profile.resolve()/"system/lib64"/(name+".so") for name in ["libhardware","libsync","liblog","libcutils","libnativewindow"]]
    cmake=(args.source_cache/"CMakeLists.txt").read_text()
    cmake=cmake.replace("elseif(LINUX OR FUCHSIA)","elseif(LINUX OR ANDROID OR FUCHSIA)",1)
    start=cmake.index("add_subdirectory(${THIRD_PARTY_DIR}/llvm-subzero")
    end=cmake.index("\n",cmake.index("set_target_properties(subzero PROPERTIES",start))
    cmake=cmake[:start]+"if(${REACTOR_BACKEND} STREQUAL \"Subzero\")\n"+cmake[start:end]+"\nendif()"+cmake[end:]
    cmake=cmake.replace('elseif(FUCHSIA)\n    target_compile_definitions(vk_base INTERFACE "VK_USE_PLATFORM_FUCHSIA")',
        'elseif(ANDROID)\n    target_compile_definitions(vk_base INTERFACE "VK_USE_PLATFORM_ANDROID_KHR" "SWIFTSHADER_EXTERNAL_MEMORY_ANDROID_HARDWARE_BUFFER=1")\n    set(OS_LIBS dl log)\nelseif(FUCHSIA)\n    target_compile_definitions(vk_base INTERFACE "VK_USE_PLATFORM_FUCHSIA")',1)
    cmake=cmake.replace("add_subdirectory(src/System)","include_directories(SYSTEM "+" ".join('"'+str(p)+'"' for p in includes)+")\nadd_subdirectory(src/System)",1)
    cmake+='\nif(ANDROID)\n target_link_options(vk_swiftshader PRIVATE "-Wl,--as-needed")\n target_link_libraries(vk_swiftshader PRIVATE '+" ".join('"'+str(p)+'"' for p in libraries)+')\n target_sources(vk_swiftshader PRIVATE src/Vulkan/VkDeviceMemoryExternalAndroid.cpp)\nendif()\n'
    write(source/"CMakeLists.txt",cmake.encode())
    path="src/System/CMakeLists.txt"
    write(source/path,(originals[path].read_text()+'\nif(ANDROID)\n target_sources(vk_system PRIVATE GrallocAndroid.cpp)\nendif()\n').encode())
    path="src/Vulkan/CMakeLists.txt"
    write(source/path,originals[path].read_text().replace('elseif(LINUX)\n    set(VULKAN_API_LIBRARY_NAME','elseif(ANDROID)\n    set(VULKAN_API_LIBRARY_NAME "vulkan.pastel.so")\nelseif(LINUX)\n    set(VULKAN_API_LIBRARY_NAME',1).encode())
    path="src/Vulkan/vk_swiftshader.lds"
    write(source/path,originals[path].read_text().replace("global:","global:\n HMI;",1).encode())
    write(source/"src/commit.h",('#define SWIFTSHADER_VERSION_STRING "'+pins["release"]+'"\n').encode())
    command=["cmake","-G","Ninja","-S",str(source),"-B",str(output/"build"),
        "-DCMAKE_TOOLCHAIN_FILE="+str(args.ndk.resolve()/"build/cmake/android.toolchain.cmake"),"-DANDROID_ABI=arm64-v8a",
        "-DANDROID_PLATFORM=android-33","-DANDROID_STL=c++_static","-DCMAKE_BUILD_TYPE=Release",
        "-DSWIFTSHADER_BUILD_TESTS=OFF","-DSWIFTSHADER_BUILD_BENCHMARKS=OFF","-DSWIFTSHADER_WARNINGS_AS_ERRORS=OFF","-DCMAKE_POLICY_VERSION_MINIMUM=3.5"]
    commands=[command,["cmake","--build",str(output/"build"),"--target","vk_swiftshader","--parallel",str(args.jobs)]]
    for index,command in enumerate(commands):
        with (output/("configure-formal.log" if index==0 else "build-formal.log")).open("w") as log:subprocess.run(command,stdout=log,stderr=log,check=True)
    host={"Darwin":"darwin-x86_64","Linux":"linux-x86_64"}[platform.system()]
    compiler=args.ndk.resolve()/"toolchains/llvm/prebuilt"/host/"bin/aarch64-linux-android33-clang"
    command=[str(compiler),"-O2","-Wall","-Wextra","-Werror","-fPIE","-pie","-I",str(output/"private")]
    for path in ["hardware/libhardware/include","system/core/libcutils/include","system/core/libsystem/include"]:command += ["-isystem",str(headers/path)]
    command += [str(SOURCE/"graphics/vulkan-probe.c"),"-ldl","-o",str(output/"vulkan-probe")]
    subprocess.run(command,check=True);commands.append(command)
    shutil.copy2(output/"build/libvk_swiftshader.so",output/"vulkan.pastel.so")
    record={"kind":"android-software-vulkan-v1","commands":commands,"pins":pins,"platform_headers":header_pin,
        "sources":{name:sha256(SOURCE/name) for name in ["build-software-vulkan.py","graphics/swiftshader-inputs.json","graphics/vulkan-probe.c"]},
        "ported_sources_sha256":tree(source)[0],"libraries":{str(p.relative_to(args.boot_profile.resolve())):sha256(p) for p in libraries},
        "ndk":(args.ndk/"source.properties").read_text(),"compiler":subprocess.check_output([str(compiler),"--version"],text=True),
        "files":{name:sha256(output/name) for name in ["vulkan.pastel.so","vulkan-probe"]},
        "needed":elf_needed(output/"vulkan.pastel.so"),"readiness":"not executed"}
    (output/"build.json").write_text(json.dumps(record,indent=2)+"\n")
    print("Built Android software Vulkan HAL and transfer probe; display composition is not asserted")


if __name__=="__main__":main()
