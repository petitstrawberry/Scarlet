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
           "android.hardware.graphics.mapper@2.0","android.hardware.graphics.allocator@2.0",
           "android.hardware.graphics.mapper@3.0","android.hardware.graphics.mapper@4.0",
           "android.hardware.graphics.composer@2.1","libfmq"]


def generate_dynamic_framebuffer_adapter(output):
    source=output/"headers/composer-utils/hwc2onfbadapter"
    target=output/"generated-hwc2onfbadapter"
    target.mkdir()
    header=(source/"include/hwc2onfbadapter/HWC2OnFbAdapter.h").read_text()
    header=header.replace(
        "    void enableVsync(bool enable);\n",
        "    void enableVsync(bool enable);\n    void setHotplugCallback(HWC2_PFN_HOTPLUG callback, hwc2_callback_data_t data);\n"
        "    void setRefreshCallback(HWC2_PFN_REFRESH callback, hwc2_callback_data_t data);\n"
        "    void updateDisplayInfo(uint32_t width, uint32_t height, int xdpi_scaled,\n"
        "                           int ydpi_scaled);\n",1)
    header=header.replace(
        "    Info mFbInfo{};\n",
        "    Info mFbInfo{};\n    hwc2_config_t mConfigId{0};\n    hwc2_config_t mNextConfigId{1};\n"
        "    mutable std::mutex mModesMutex;\n    std::mutex mBufferMutex;\n"
        "    HWC2_PFN_HOTPLUG mHotplugCallback{nullptr};\n"
        "    hwc2_callback_data_t mHotplugData{nullptr};\n"
        "    HWC2_PFN_REFRESH mRefreshCallback{nullptr};\n"
        "    hwc2_callback_data_t mRefreshData{nullptr};\n",1)
    header=header.replace("    static hwc2_config_t getConfigId();\n",
                          "    hwc2_config_t getConfigId();\n",1)
    header=header.replace(
        "    const Info& getInfo() const;\n",
        "    Info getInfo() const;\n    Info getConfigInfo(hwc2_config_t config) const;\n"
        "    bool setConfig(hwc2_config_t config);\n",1)
    implementation=(source/"HWC2OnFbAdapter.cpp").read_text()
    implementation=implementation.replace("#include <algorithm>\n","#include <algorithm>\n#include <cstdio>\n",1)
    implementation=implementation.replace(
        "        case HWC2_CALLBACK_HOTPLUG:\n            if (pointer) {\n"
        "                reinterpret_cast<HWC2_PFN_HOTPLUG>(pointer)(callbackData, adapter.getDisplayId(),\n"
        "                                                            HWC2_CONNECTION_CONNECTED);\n"
        "            }\n",
        "        case HWC2_CALLBACK_HOTPLUG:\n"
        "            adapter.setHotplugCallback(reinterpret_cast<HWC2_PFN_HOTPLUG>(pointer), callbackData);\n"
        "            if (pointer) {\n"
        "                reinterpret_cast<HWC2_PFN_HOTPLUG>(pointer)(callbackData, adapter.getDisplayId(),\n"
        "                                         HWC2_CONNECTION_CONNECTED);\n"
        "            }\n",1)
    implementation=implementation.replace(
        "        case HWC2_CALLBACK_REFRESH:\n            break;\n",
        "        case HWC2_CALLBACK_REFRESH:\n"
        "            adapter.setRefreshCallback(reinterpret_cast<HWC2_PFN_REFRESH>(pointer), callbackData);\n"
        "            if (pointer) {\n"
        "                reinterpret_cast<HWC2_PFN_REFRESH>(pointer)(callbackData, adapter.getDisplayId());\n"
        "            }\n"
        "            break;\n",1)
    implementation=implementation.replace(
        "hwc2_config_t HWC2OnFbAdapter::getConfigId() {\n    return 0;\n}\n",
        "hwc2_config_t HWC2OnFbAdapter::getConfigId() {\n"
        "    std::lock_guard<std::mutex> lock(mModesMutex);\n    return mConfigId;\n}\n",1)
    old_configs="""    if (outConfigs) {
        if (*outNumConfigs > 0) {
            outConfigs[0] = adapter.getConfigId();
            *outNumConfigs = 1;
        }
    } else {
        *outNumConfigs = 1;
    }
"""
    new_configs="""    if (outConfigs) {
        if (*outNumConfigs > 0) {
            outConfigs[0] = adapter.getConfigId();
            *outNumConfigs = 1;
        }
    } else {
        *outNumConfigs = 1;
    }
"""
    if implementation.count(old_configs)!=1:raise ValueError("changed HWC2 display config hook")
    implementation=implementation.replace(old_configs,new_configs,1)
    old_attribute="""    if (adapter.getConfigId() != config) {
        return HWC2_ERROR_BAD_CONFIG;
    }

    const auto& info = adapter.getInfo();
"""
    new_attribute="""    const auto& info = adapter.getConfigInfo(config);
    if (info.width == 0 || info.height == 0) {
        return HWC2_ERROR_BAD_CONFIG;
    }
"""
    if implementation.count(old_attribute)!=1:raise ValueError("changed HWC2 attribute hook")
    implementation=implementation.replace(old_attribute,new_attribute,1)
    old_set="""    if (adapter.getConfigId() != config) {
        return HWC2_ERROR_BAD_CONFIG;
    }

    return HWC2_ERROR_NONE;
}
"""
    new_set="""    return adapter.setConfig(config) ? HWC2_ERROR_NONE : HWC2_ERROR_BAD_CONFIG;
}
"""
    if implementation.count(old_set)!=1:raise ValueError("changed HWC2 set config hook")
    implementation=implementation.replace(old_set,new_set,1)
    old_present="""    adapter.postBuffer();
    *outPresentFence = -1;

    return HWC2_ERROR_NONE;
}
"""
    new_present="""    if (!adapter.postBuffer()) {
        return HWC2_ERROR_UNSUPPORTED;
    }
    *outPresentFence = -1;

    return HWC2_ERROR_NONE;
}
"""
    if implementation.count(old_present)!=1:raise ValueError("changed HWC2 present hook")
    implementation=implementation.replace(old_present,new_present,1)
    old_buffer="""void HWC2OnFbAdapter::setBuffer(buffer_handle_t buffer) {
    if (mFbDevice->compositionComplete) {
        mFbDevice->compositionComplete(mFbDevice);
    }
    mBuffer = buffer;
}

bool HWC2OnFbAdapter::postBuffer() {
    int error = 0;
    if (mBuffer) {
        error = mFbDevice->post(mFbDevice, mBuffer);
    }

    return error == 0;
}
"""
    new_buffer="""void HWC2OnFbAdapter::setBuffer(buffer_handle_t buffer) {
    if (mFbDevice->compositionComplete) {
        mFbDevice->compositionComplete(mFbDevice);
    }
    std::lock_guard<std::mutex> lock(mBufferMutex);
    mBuffer = buffer;
}

bool HWC2OnFbAdapter::postBuffer() {
    std::lock_guard<std::mutex> lock(mBufferMutex);
    int error = 0;
    if (mBuffer) {
        error = mFbDevice->post(mFbDevice, mBuffer);
    }

    return error == 0;
}
"""
    if implementation.count(old_buffer)!=1:raise ValueError("changed HWC2 framebuffer buffer hooks")
    implementation=implementation.replace(old_buffer,new_buffer,1)
    marker="const HWC2OnFbAdapter::Info& HWC2OnFbAdapter::getInfo() const {\n    return mFbInfo;\n}\n"
    addition=""
    addition+="\nHWC2OnFbAdapter::Info HWC2OnFbAdapter::getInfo() const {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n    return mFbInfo;\n}\n"
    addition+="HWC2OnFbAdapter::Info HWC2OnFbAdapter::getConfigInfo(hwc2_config_t config) const {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n"
    addition+="    return config == mConfigId ? mFbInfo : Info{};\n}\n"
    addition+="bool HWC2OnFbAdapter::setConfig(hwc2_config_t config) {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n"
    addition+="    if (config != mConfigId) return false;\n"
    addition+="    mState = State::MODIFIED;\n    return true;\n}\n"
    addition+="\nvoid HWC2OnFbAdapter::updateDisplayInfo(uint32_t width, uint32_t height,\n"
    addition+="                                             int xdpi_scaled, int ydpi_scaled) {\n"
    addition+="    HWC2_PFN_HOTPLUG hotplug = nullptr;\n"
    addition+="    hwc2_callback_data_t hotplug_data = nullptr;\n"
    addition+="    {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n"
    addition+="    if (width == mFbInfo.width && height == mFbInfo.height &&\n"
    addition+="        xdpi_scaled == mFbInfo.xdpi_scaled && ydpi_scaled == mFbInfo.ydpi_scaled) return;\n"
    addition+="    mConfigId = mNextConfigId;\n    mNextConfigId = mNextConfigId == UINT32_MAX ? 1 : mNextConfigId + 1;\n"
    addition+="    mFbInfo.width = width;\n    mFbInfo.height = height;\n"
    addition+="    mFbInfo.xdpi_scaled = xdpi_scaled;\n    mFbInfo.ydpi_scaled = ydpi_scaled;\n"
    addition+="    mState = State::MODIFIED;\n"
    addition+="    hotplug = mHotplugCallback;\n    hotplug_data = mHotplugData;\n"
    addition+="    std::fprintf(stderr, \"SCARLET_ANDROID_HWC_CONFIG config=%u hotplug=%p\\n\",\n"
    addition+="                 mConfigId, reinterpret_cast<void*>(hotplug));\n"
    addition+="    }\n"
    addition+="    { std::lock_guard<std::mutex> lock(mBufferMutex); mBuffer = nullptr; }\n"
    addition+="    if (hotplug) hotplug(hotplug_data, getDisplayId(), HWC2_CONNECTION_CONNECTED);\n"
    addition+="}\n"
    addition+="\nvoid HWC2OnFbAdapter::setHotplugCallback(HWC2_PFN_HOTPLUG callback, hwc2_callback_data_t data) {\n"
    addition+="    mHotplugCallback = callback;\n    mHotplugData = data;\n}\n"
    addition+="\nvoid HWC2OnFbAdapter::setRefreshCallback(HWC2_PFN_REFRESH callback, hwc2_callback_data_t data) {\n"
    addition+="    mRefreshCallback = callback;\n    mRefreshData = data;\n}\n"
    if implementation.count(marker)!=1:raise ValueError("changed HWC2 framebuffer adapter info hook")
    implementation=implementation.replace(marker,addition,1)
    (target/"HWC2OnFbAdapter.cpp").write_text(implementation)
    include=target/"include/hwc2onfbadapter";include.mkdir(parents=True)
    (include/"HWC2OnFbAdapter.h").write_text(header)
    return target


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
            extract_headers(data,pin["tree_sha256"],target,max_total=256*1024**2,
                            max_files=100000,max_file=16*1024**2)
    host={"Darwin":"darwin-x86_64","Linux":"linux-x86_64"}[platform.system()]
    compiler=args.ndk.resolve()/"toolchains/llvm/prebuilt"/host/"bin/aarch64-linux-android33-clang++"
    flags=[str(compiler),"-std=c++17","-O2","-fPIC","-fno-rtti","-fno-exceptions","-nostdinc++","-nostdlib++","-Wl,-z,defs","-Wl,--as-needed"]
    vndk=output/"headers/vndk33"
    includes=["external/libcxx/include","system/libhidl/base/include","system/libhidl/transport/include",
              "system/libhwbinder/include","system/libfmq/base","system/libfmq/include","system/core/libutils/include",
              "system/core/libcutils/include","system/core/libcutils/include_outside_system",
              "system/core/libsystem/include","system/libbase/include","system/logging/liblog/include_vndk",
              "hardware/libhardware/include","system/core/libsync/include"]
    for path in includes:flags += ["-isystem",str(vndk/path)]
    # Full qualified generated HIDL include names keep interface versions distinct.
    for path in sorted((vndk/"generated-headers").rglob("gen")):flags += ["-isystem",str(path)]
    for path in sorted((output/"headers/composer21-headers").rglob("gen")):flags += ["-isystem",str(path)]
    for name in ["mapper-utils","allocator-utils"]:
        for kind in ["hal","passthrough"]:flags += ["-isystem",str(output/"headers"/name/kind/"include")]
    for kind in ["hal","passthrough","resources","command-buffer"]:
        flags += ["-isystem",str(output/"headers/composer-utils"/kind/"include")]
    dynamic_adapter=generate_dynamic_framebuffer_adapter(output)
    flags += ["-isystem",str(dynamic_adapter/"include")]
    libraries=[]
    for name in LIBRARIES:
        candidates=[args.boot_profile.resolve()/partition/"lib64"/(name+".so") for partition in ["system","vendor"]]
        libraries.append(next((p for p in candidates if p.is_file()),candidates[0]))
    link=[str(path) for path in libraries]
    products={"android.hardware.graphics.mapper@2.0-impl.scarlet.so":("mapper.cpp",["-shared"]),
              "android.hardware.graphics.allocator@2.0-service.scarlet":("allocator-service.cpp",["-pie"]),
              "android.hardware.graphics.composer@2.1-service.scarlet":("composer-service.cpp",["-pie"]),
              "graphics-adapter-probe":("adapter-probe.cpp",["-pie"])}
    commands=[]
    for name,(source,extra) in products.items():
        command=flags+extra+[str(SOURCE/"graphics"/source),*link,"-o",str(output/name)]
        if source=="composer-service.cpp":
            command[1:1]=[str(dynamic_adapter/"HWC2OnFbAdapter.cpp"),str(output/"headers/composer-utils/resources/ComposerResources.cpp")]
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
