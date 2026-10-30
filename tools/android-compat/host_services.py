"""Explicit stock services selected for the hosted Android environment."""
import hashlib

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
STOCK_TEXT = ("service system_suspend /" + BINARY + "\n"
              "    class early_hal\n    user system\n    group system wakelock\n"
              "    capabilities BLOCK_SUSPEND\n")


def validate_selection(profile):
    names = profile.get("host_services", [])
    if (not isinstance(names,list) or len(names)>4 or
            any(name not in (SUSPEND,HW_MANAGER,GRAPHICS,INSTALLER) for name in names) or len(set(names))!=len(names)):
        raise ValueError("unsupported stock host service selection")
    selected = CONTRACT in profile.get("vintf", {}).get("framework", [])
    if selected != (SUSPEND in names):
        raise ValueError("SystemSuspend requires both its host provider and VINTF contract")
    from vintf_inputs import graphics_selection
    device=profile.get("vintf",{}).get("device",[])
    if (GRAPHICS in names and (HW_MANAGER not in names or not graphics_selection(device)) or
            GRAPHICS not in names and device):raise ValueError("graphics services require their exact device inventory and hardware-service manager")
    return names


def imports(profile):
    return ["/"+{SUSPEND:MANAGED_RC,HW_MANAGER:HW_MANAGED_RC,GRAPHICS:GRAPHICS_RC,INSTALLER:INSTALLER_MANAGED_RC}[name] for name in validate_selection(profile)]


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
    if GRAPHICS in names:
        record=proof[GRAPHICS]
        if record.get("backend")!="cpu-gralloc0" or not record.get("files"):
            raise ValueError("missing CPU graphics provider inputs")
        for path,digest in record["files"].items():
            if metadata["files"].get(path)!=digest:raise ValueError("changed CPU graphics provider inputs")
    if INSTALLER in names:
        record=proof[INSTALLER]
        if (metadata["files"].get(INSTALLER_BINARY)!=record.get("binary_sha256") or
                metadata["files"].get(INSTALLER_MANAGED_RC)!=record.get("managed_rc_sha256") or
                record.get("source_rc_sha256")!=record.get("managed_rc_sha256")):
            raise ValueError("changed stock installer inputs")
