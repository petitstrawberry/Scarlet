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
        "    void setRefreshCallback(HWC2_PFN_REFRESH callback, hwc2_callback_data_t data);\n    void updateDisplayInfo();\n",1)
    header=header.replace(
        "    Info mFbInfo{};\n",
        "    struct Config { hwc2_config_t id; Info info; };\n"
        "    Info mFbInfo{};\n    hwc2_config_t mConfigId{0};\n    hwc2_config_t mNextConfigId{1};\n"
        "    std::vector<Config> mConfigs;\n"
        "    mutable std::mutex mModesMutex;\n    HWC2_PFN_HOTPLUG mHotplugCallback{nullptr};\n"
        "    hwc2_callback_data_t mHotplugData{nullptr};\n"
        "    HWC2_PFN_REFRESH mRefreshCallback{nullptr};\n"
        "    hwc2_callback_data_t mRefreshData{nullptr};\n",1)
    header=header.replace("    static hwc2_config_t getConfigId();\n",
                          "    hwc2_config_t getConfigId();\n",1)
    header=header.replace(
        "    const Info& getInfo() const;\n",
        "    Info getInfo() const;\n    size_t getConfigCount() const;\n"
        "    hwc2_config_t getConfigIdAt(size_t index) const;\n"
        "    Info getConfigInfo(hwc2_config_t config) const;\n"
        "    bool setConfig(hwc2_config_t config);\n",1)
    header=header.replace("#include <unordered_set>\n","#include <unordered_set>\n#include <vector>\n",1)
    implementation=(source/"HWC2OnFbAdapter.cpp").read_text()
    implementation=implementation.replace("#include <algorithm>\n","#include <algorithm>\n#include <cstdio>\n",1)
    implementation=implementation.replace(
        "    mFbInfo.ydpi_scaled = int(mFbDevice->ydpi * 1000.0f);\n",
        "    mFbInfo.ydpi_scaled = int(mFbDevice->ydpi * 1000.0f);\n"
        "    mConfigs.push_back({mConfigId, mFbInfo});\n",1)
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
    new_configs="""    const uint32_t count = adapter.getConfigCount();
    if (outConfigs) {
        const uint32_t written = std::min<uint32_t>(*outNumConfigs, count);
        for (uint32_t i = 0; i < written; ++i) outConfigs[i] = adapter.getConfigIdAt(i);
        *outNumConfigs = written;
    } else {
        *outNumConfigs = count;
    }
"""
    if implementation.count(old_configs)!=1:raise ValueError("changed HWC2 display config hook")
    implementation=implementation.replace(old_configs,new_configs,1)
    old_attribute="""    if (adapter.getConfigId() != config) {
        return HWC2_ERROR_BAD_CONFIG;
    }

    const auto& info = adapter.getInfo();
"""
    new_attribute="""    if (adapter.getConfigId() != config) {
        return HWC2_ERROR_BAD_CONFIG;
    }

    const auto& info = adapter.getConfigInfo(config);
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
    marker="const HWC2OnFbAdapter::Info& HWC2OnFbAdapter::getInfo() const {\n    return mFbInfo;\n}\n"
    addition=""
    addition+="\nHWC2OnFbAdapter::Info HWC2OnFbAdapter::getInfo() const {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n    return mFbInfo;\n}\n"
    addition+="size_t HWC2OnFbAdapter::getConfigCount() const {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n    return mConfigs.size();\n}\n"
    addition+="hwc2_config_t HWC2OnFbAdapter::getConfigIdAt(size_t index) const {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n    return index < mConfigs.size() ? mConfigs[index].id : 0;\n}\n"
    addition+="HWC2OnFbAdapter::Info HWC2OnFbAdapter::getConfigInfo(hwc2_config_t config) const {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n"
    addition+="    for (const auto& entry : mConfigs) if (entry.id == config) return entry.info;\n"
    addition+="    return Info{};\n}\n"
    addition+="bool HWC2OnFbAdapter::setConfig(hwc2_config_t config) {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n"
    addition+="    for (const auto& entry : mConfigs) if (entry.id == config) { mConfigId = config; mFbInfo = entry.info; break; }\n"
    addition+="    if (config != mConfigId) return false;\n"
    addition+="    mState = State::MODIFIED;\n    return true;\n}\n"
    addition+="\nvoid HWC2OnFbAdapter::updateDisplayInfo() {\n"
    addition+="    const uint32_t width = mFbDevice->width;\n"
    addition+="    const uint32_t height = mFbDevice->height;\n"
    addition+="    HWC2_PFN_HOTPLUG hotplug = nullptr;\n"
    addition+="    hwc2_callback_data_t hotplug_data = nullptr;\n"
    addition+="    HWC2_PFN_REFRESH refresh = nullptr;\n"
    addition+="    hwc2_callback_data_t refresh_data = nullptr;\n"
    addition+="    {\n"
    addition+="    std::lock_guard<std::mutex> lock(mModesMutex);\n"
    addition+="    if (width == mFbInfo.width && height == mFbInfo.height) return;\n"
    addition+="    mConfigId = mNextConfigId;\n    mNextConfigId = mNextConfigId == UINT32_MAX ? 1 : mNextConfigId + 1;\n"
    addition+="    mFbInfo.width = width;\n    mFbInfo.height = height;\n"
    addition+="    if (mConfigs.size() == 16) mConfigs.erase(mConfigs.begin());\n"
    addition+="    mConfigs.push_back({mConfigId, mFbInfo});\n"
    addition+="    mState = State::MODIFIED;\n"
    addition+="    hotplug = mHotplugCallback;\n    hotplug_data = mHotplugData;\n"
    addition+="    refresh = mRefreshCallback;\n    refresh_data = mRefreshData;\n"
    addition+="    std::fprintf(stderr, \"SCARLET_ANDROID_HWC_CONFIG config=%u hotplug=%p refresh=%p\\n\",\n"
    addition+="                 mConfigId, reinterpret_cast<void*>(hotplug), reinterpret_cast<void*>(refresh));\n"
    addition+="    }\n"
    addition+="    if (hotplug) hotplug(hotplug_data, getDisplayId(), HWC2_CONNECTION_CONNECTED);\n"
    addition+="    if (refresh) refresh(refresh_data, getDisplayId());\n}\n"
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
