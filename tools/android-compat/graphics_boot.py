"""Verified CPU graphics providers composed by the Android userspace manager."""
import json
from pathlib import Path
import shutil

from art_inputs import sha256
from host_services import GRAPHICS_RC
from vintf_inputs import GRAPHICS_DEVICE

SOURCE=Path(__file__).resolve().parent
ALLOCATOR="vendor/bin/hw/android.hardware.graphics.allocator@2.0-service.scarlet"
COMPOSER="vendor/bin/hw/android.hardware.graphics.composer@2.1-service.scarlet"
MAPPER="vendor/lib64/hw/android.hardware.graphics.mapper@2.0-impl.scarlet.so"
SURFACEFLINGER="system/bin/surfaceflinger"


def prepare(inputs,metadata,system,vendor,builds,temporary):
    files={};records={}
    def install(source,name):
        target=inputs/name
        previous=metadata.get("host_service_inputs",{}).get("graphics_cpu",{}).get("files",{}).get(name)
        if name in metadata["symlinks"] or target.exists() and sha256(target) not in [sha256(source),previous]:
            raise ValueError("graphics provider conflicts with existing boot input: "+name)
        target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,target)
        if sha256(target)!=sha256(source):raise ValueError("changed graphics provider during staging")
        files[name]=sha256(target)
    selections=[("hal","android-cpu-graphics-hal-v1"),("adapters","android-stock-graphics-adapters-v1"),("client","android-sws-client-v1")]
    if "vulkan" in builds:selections.append(("vulkan","android-software-vulkan-v1"))
    if ("vulkan" in builds)!=(metadata["profile"]["boot_properties"].get("ro.hardware.vulkan")=="pastel"):
        raise ValueError("software Vulkan requires both its selected driver and verified build")
    for name,kind in selections:
        directory=builds[name].resolve();record=json.loads((directory/"build.json").read_text())
        if record.get("kind")!=kind:raise ValueError("unexpected graphics provider build kind")
        for path,digest in record["files"].items():
            if sha256(directory/path)!=digest:raise ValueError("changed graphics provider build")
        for path,digest in record["sources"].items():
            actual=SOURCE.parent.parent/path if name=="client" else SOURCE/path
            if sha256(actual)!=digest:raise ValueError("graphics source changed; rebuild provider")
        for path,digest in record.get("libraries",{}).items():
            destination=inputs/path
            if not destination.exists():
                image,original=(vendor,"/"+path.removeprefix("vendor/")) if path.startswith("vendor/") else (system,"/"+path)
                image.dump(original,destination);files[path]=sha256(destination)
            if sha256(inputs/path)!=digest:raise ValueError("graphics adapter platform library mismatch")
        records[name]={"build_sha256":sha256(directory/"build.json"),"files":record["files"]}
    for source,name in [(builds["hal"]/"gralloc.scarlet.so","vendor/lib64/hw/gralloc.scarlet.so"),
                        (builds["adapters"]/Path(ALLOCATOR).name,ALLOCATOR),
                        (builds["adapters"]/Path(COMPOSER).name,COMPOSER),
                        (builds["adapters"]/Path(MAPPER).name,MAPPER),
                        (builds["client"]/"libsws_client_c.so","vendor/lib64/libsws_client_c.so")]:install(source,name)
    if "vulkan" in builds:install(builds["vulkan"]/"vulkan.pastel.so","vendor/lib64/hw/vulkan.pastel.so")
    # Composer HIDL is not part of the VNDK namespace shared with vendor.
    # Package the exact linked interface DSO alongside its vendor provider.
    interface="system/lib64/android.hardware.graphics.composer@2.1.so"
    install(inputs/interface,"vendor/lib64/android.hardware.graphics.composer@2.1.so")
    rc_sources={}
    for name,image,path in [("surfaceflinger",system,"/system/etc/init/surfaceflinger.rc")]:
        source=temporary/(name+".rc");image.dump(path,source)
        text=source.read_text()
        if text.count("    capabilities SYS_NICE\n")!=1:raise ValueError("changed graphics service capability request")
        rc_sources[name]={"sha256":sha256(source),"omitted_capabilities":[]}
        rc_sources[name]["managed"]=text
    allocator=("service vendor.graphics-allocator-2-0 /"+ALLOCATOR+"\n    class hal\n    user system\n"
               "    group graphics\n    task_profiles ServiceCapacityLow\n")
    composer=("service vendor.graphics-composer-2-1 /"+COMPOSER+"\n    class hal animation\n    user system\n"
              "    group graphics drmrpc\n    task_profiles ServiceCapacityLow\n    onrestart restart surfaceflinger\n")
    rc=temporary/"graphics_cpu.rc"
    rc.write_text("# CPU graphics providers selected by the Scarlet manager\n"+allocator+"\n"+
                  composer+"\n"+rc_sources["surfaceflinger"].pop("managed")+
                  "\non init\n"+"".join("    mkdir /dev/socket/"+p+" 0755 system graphics\n" for p in
                      ["pdx","pdx/system","pdx/system/vr","pdx/system/vr/display"])+
                  "    start vendor.graphics-allocator-2-0\n    start vendor.graphics-composer-2-1\n    start surfaceflinger\n")
    previous=metadata.get("host_service_inputs",{}).get("graphics_cpu",{}).get("files",{}).get(GRAPHICS_RC)
    old=inputs/GRAPHICS_RC
    if old.exists() and sha256(old)!=previous:raise ValueError("changed manager graphics configuration")
    if old.exists():old.unlink()
    install(rc,GRAPHICS_RC)
    providers={}
    for contract,path in [("android.hardware.graphics.allocator@2.0::IAllocator/default",ALLOCATOR),
                          ("android.hardware.graphics.mapper@2.0::IMapper/default",MAPPER),
                          ("android.hardware.graphics.composer@2.1::IComposer/default",COMPOSER)]:
        providers[contract]={"transport":GRAPHICS_DEVICE[contract],"path":path}
    proof={"backend":"cpu-gralloc0","files":files,"builds":records,"stock_rc":rc_sources,
           "relocations":{"vendor/lib64/android.hardware.graphics.composer@2.1.so":interface}}
    native_libraries=set()
    for build in builds.values():native_libraries.update(json.loads((build/"build.json").read_text()).get("libraries",{}))
    return proof,providers,[SURFACEFLINGER,*sorted(native_libraries)]
