"""Explicit stock services selected for the hosted Android environment."""
import hashlib
import xml.etree.ElementTree as ET

SUSPEND = "system_suspend"
BINARY = "system/bin/hw/android.system.suspend@1.0-service"
STOCK_RC = "system/etc/init/android.system.suspend@1.0-service.rc"
MANAGED_RC = "scarlet/boot/services/system_suspend.rc"
CONTRACT = "android.system.suspend.ISystemSuspend/default"
HW_MANAGER="hwservicemanager"
HW_BINARY="system/bin/hwservicemanager"
HW_STOCK_RC="system/etc/init/hwservicemanager.rc"
HW_MANAGED_RC="scarlet/boot/services/hwservicemanager.rc"
GRAPHICS="graphics_cpu"
GRAPHICS_RC="scarlet/boot/services/graphics_cpu.rc"
INSTALLER="installd"
INSTALLER_BINARY="system/bin/installd"
INSTALLER_STOCK_RC="system/etc/init/installd.rc"
INSTALLER_MANAGED_RC="scarlet/boot/services/installd.rc"
ODSIGN="odsign"
ODSIGN_BINARY="system/bin/odsign"
ODSIGN_STOCK_RC="system/etc/init/odsign.rc"
ODSIGN_MANAGED_RC="scarlet/boot/services/odsign.rc"
HEALTH="health"
HEALTH_BINARY="vendor/bin/hw/android.hardware.health@2.1-service"
HEALTH_IMPL="vendor/lib64/hw/android.hardware.health@2.0-impl-2.1.so"
HEALTH_STOCK_RC="vendor/etc/init/android.hardware.health@2.1-service.rc"
HEALTH_MANAGED_RC="scarlet/boot/services/health.rc"
HEALTH_MANIFEST="vendor/etc/vintf/manifest/android.hardware.health@2.1.xml"
HEALTH_TEXT=("service health-hal-2-1 /"+HEALTH_BINARY+"\n"
             "    class hal charger\n    user system\n    group system\n"
             "    capabilities WAKE_ALARM BLOCK_SUSPEND\n    file /dev/kmsg w\n")
STOCK_TEXT = ("service system_suspend /" + BINARY + "\n"
              "    class early_hal\n    user system\n    group system wakelock\n"
              "    capabilities BLOCK_SUSPEND\n")


def validate_selection(profile):
    names = profile.get("host_services", [])
    if (not isinstance(names,list) or len(names)>7 or
            any(name not in (SUSPEND,HW_MANAGER,GRAPHICS,INSTALLER,ODSIGN,HEALTH) for name in names) or len(set(names))!=len(names)):
        raise ValueError("unsupported stock host service selection")
    selected = CONTRACT in profile.get("vintf", {}).get("framework", [])
    if selected != (SUSPEND in names):
        raise ValueError("SystemSuspend requires both its host provider and VINTF contract")
    from vintf_inputs import graphics_selection, HEALTH_CONTRACT, FRAMEWORK_HIDL
    if FRAMEWORK_HIDL.intersection(profile.get("vintf",{}).get("framework",[])) and HW_MANAGER not in names:
        raise ValueError("HIDL framework management requires its stock provider")
    device=profile.get("vintf",{}).get("device",[])
    graphics=[name for name in device if name!=HEALTH_CONTRACT]
    if (GRAPHICS in names and (HW_MANAGER not in names or not graphics_selection(graphics)) or
            GRAPHICS not in names and graphics):raise ValueError("graphics services require their exact device inventory and hardware-service manager")
    if ((HEALTH in names)!=(HEALTH_CONTRACT in device) or
            HEALTH in names and HW_MANAGER not in names):
        raise ValueError("Health requires its exact device contract and hardware-service manager")
    return names


def imports(profile):
    return ["/"+{SUSPEND:MANAGED_RC,HW_MANAGER:HW_MANAGED_RC,GRAPHICS:GRAPHICS_RC,INSTALLER:INSTALLER_MANAGED_RC,ODSIGN:ODSIGN_MANAGED_RC,HEALTH:HEALTH_MANAGED_RC}[name] for name in validate_selection(profile)]


def build_health(stock_rc,binary,manifest):
    from vintf_inputs import HEALTH_CONTRACT
    if stock_rc.decode()!=HEALTH_TEXT:raise ValueError("changed stock Health service configuration")
    root=ET.fromstring(manifest)
    hals=root.findall("hal")
    if (root.tag!="manifest" or root.get("type")!="device" or len(hals)!=1 or
            hals[0].get("format")!="hidl" or hals[0].findtext("name")!="android.hardware.health" or
            hals[0].findtext("transport")!="hwbinder" or
            [x.text for x in hals[0].findall("fqname")]!=["@2.1::IHealth/default"]):
        raise ValueError("changed stock Health device contract")
    managed=("# Read-only native telemetry; host wake and suspend remain external.\n"+
             HEALTH_TEXT.replace("    capabilities WAKE_ALARM BLOCK_SUSPEND\n", "")).encode()
    digest=lambda data:hashlib.sha256(data).hexdigest()
    proof={"source_rc":HEALTH_STOCK_RC,"source_rc_sha256":digest(stock_rc),
           "binary_sha256":digest(binary),"managed_rc_sha256":digest(managed),
           "source_manifest":HEALTH_MANIFEST,"source_manifest_sha256":digest(manifest),
           "omitted_capabilities":["WAKE_ALARM","BLOCK_SUSPEND"],"host_suspend":False}
    provider={HEALTH_CONTRACT:{"transport":"hwbinder","path":HEALTH_BINARY,"sha256":digest(binary),
                              "source":HEALTH_MANIFEST,"source_sha256":digest(manifest)}}
    return managed,proof,provider


def build_odsign(stock_rc,binary):
    if not stock_rc.startswith(b"service odsign /system/bin/odsign\n    class core\n    user root\n    group system\n    disabled"):
        raise ValueError("changed stock on-device signing configuration")
    digest=lambda value:hashlib.sha256(value).hexdigest()
    return stock_rc,{"source_rc":ODSIGN_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                     "binary_sha256":digest(binary),"managed_rc_sha256":digest(stock_rc)}


def build_installer(stock_rc,binary):
    if not stock_rc.lstrip(b"\n").startswith(b"service installd /system/bin/installd\n    class main\n"):
        raise ValueError("changed stock installer service configuration")
    digest=lambda value:hashlib.sha256(value).hexdigest()
    return stock_rc,{"source_rc":INSTALLER_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                     "binary_sha256":digest(binary),"managed_rc_sha256":digest(stock_rc)}


def build_hwmanager(stock_rc,binary):
    if not stock_rc.startswith(b"service hwservicemanager /system/bin/hwservicemanager\n"):
        raise ValueError("changed stock hardware-service manager configuration")
    digest=lambda value:hashlib.sha256(value).hexdigest()
    return stock_rc,{"source_rc":HW_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                     "binary_sha256":digest(binary),"managed_rc_sha256":digest(stock_rc)}


def build_suspend(stock_rc, binary):
    # The pinned daemon explicitly supports absent host suspend sysfs using a
    # blocking socketpair, while retaining real Binder wake-lock bookkeeping.
    # Do not request a Linux capability that Scarlet cannot grant or enforce.
    if stock_rc.decode() != STOCK_TEXT:
        raise ValueError("changed stock SystemSuspend service configuration")
    managed = ("# Android wake locks only; host system suspend is not delegated.\n" +
               STOCK_TEXT.replace("    capabilities BLOCK_SUSPEND\n", "")).encode()
    digest = lambda value: hashlib.sha256(value).hexdigest()
    return managed, {"source_rc": STOCK_RC, "source_rc_sha256": digest(stock_rc),
                     "binary_sha256": digest(binary), "managed_rc_sha256": digest(managed),
                     "omitted_capabilities": ["BLOCK_SUSPEND"], "host_suspend": False}


def validate_staged(metadata):
    names = validate_selection(metadata["profile"])
    proof = metadata.get("host_service_inputs", {})
    if set(proof) != set(names):
        raise ValueError("host services require matching verified provider inputs")
    if SUSPEND in names:
        record = proof[SUSPEND]
        if (metadata["files"].get(BINARY) != record.get("binary_sha256") or
                metadata["files"].get(MANAGED_RC) != record.get("managed_rc_sha256") or
                record.get("host_suspend") is not False or
                record.get("omitted_capabilities") != ["BLOCK_SUSPEND"]):
            raise ValueError("changed SystemSuspend host provider inputs")
    if HW_MANAGER in names:
        record=proof[HW_MANAGER]
        if (metadata["files"].get(HW_BINARY)!=record.get("binary_sha256") or
                metadata["files"].get(HW_MANAGED_RC)!=record.get("managed_rc_sha256") or
                record.get("source_rc_sha256")!=record.get("managed_rc_sha256")):
            raise ValueError("changed hardware-service manager inputs")
    if HEALTH in names:
        record=proof[HEALTH]
        if (metadata["files"].get(HEALTH_BINARY)!=record.get("binary_sha256") or
                metadata["files"].get(HEALTH_MANAGED_RC)!=record.get("managed_rc_sha256") or
                not record.get("implementation_sha256") or
                metadata["files"].get(HEALTH_IMPL)!=record["implementation_sha256"] or
                record.get("host_suspend") is not False or
                record.get("omitted_capabilities")!=["WAKE_ALARM","BLOCK_SUSPEND"]):
            raise ValueError("changed Health host provider inputs")
    if GRAPHICS in names:
        record=proof[GRAPHICS]
        if record.get("backend")!="cpu-gralloc0" or not record.get("files"):
            raise ValueError("missing CPU graphics provider inputs")
        for path,digest in record["files"].items():
            if metadata["files"].get(path)!=digest:raise ValueError("changed CPU graphics provider inputs")
    for name,binary,rc in [(INSTALLER,INSTALLER_BINARY,INSTALLER_MANAGED_RC),(ODSIGN,ODSIGN_BINARY,ODSIGN_MANAGED_RC)]:
        if name not in names:continue
        record=proof[name]
        if (metadata["files"].get(binary)!=record.get("binary_sha256") or
                metadata["files"].get(rc)!=record.get("managed_rc_sha256") or
                record.get("source_rc_sha256")!=record.get("managed_rc_sha256")):
            raise ValueError("changed stock service inputs: "+name)
