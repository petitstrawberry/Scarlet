"""Explicit stock services selected for the hosted Android environment."""
import hashlib
import re
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
KEYSTORE="keystore2"
KEYSTORE_BINARY="system/bin/keystore2"
KEYSTORE_STOCK_RC="system/etc/init/keystore2.rc"
KEYSTORE_MANAGED_RC="scarlet/boot/services/keystore2.rc"
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
VOLD="vold"
VOLD_BINARY="system/bin/vold"
VOLD_STOCK_RC="system/etc/init/vold.rc"
VOLD_MANAGED_RC="scarlet/boot/services/vold.rc"
LMKD="lmkd"
LMKD_BINARY="system/bin/lmkd"
LMKD_STOCK_RC="system/etc/init/lmkd.rc"
LMKD_MANAGED_RC="scarlet/boot/services/lmkd.rc"
LMKD_TEXT=("service lmkd /system/bin/lmkd\n    class core\n    user lmkd\n"
           "    group lmkd system readproc\n"
           "    capabilities DAC_OVERRIDE KILL IPC_LOCK SYS_NICE SYS_RESOURCE\n"
           "    critical\n    socket lmkd seqpacket+passcred 0660 system system\n"
           "    task_profiles ServiceCapacityLow\n\n"
           "on property:lmkd.reinit=1\n    exec_background /system/bin/lmkd --reinit\n\n"
           "# reinitialize lmkd after device finished booting if experiments set any flags during boot\n"
           "on property:sys.boot_completed=1 && property:lmkd.reinit=0\n    setprop lmkd.reinit 1\n\n"
           "# properties most likely to be used in experiments\n"
           "# setting persist.device_config.* property either triggers immediate lmkd re-initialization\n"
           "# if the device finished booting or sets lmkd.reinit=0 to re-initialize lmkd after boot completes\n")
for _name in ("debug","kill_heaviest_task","kill_timeout_ms","swap_free_low_percentage",
              "psi_partial_stall_ms","psi_complete_stall_ms","thrashing_limit",
              "thrashing_limit_decay","thrashing_limit_critical","swap_util_max","filecache_min_kb"):
    LMKD_TEXT += "on property:persist.device_config.lmkd_native." + _name + "=*\n    setprop lmkd.reinit ${sys.boot_completed:-0}\n\n"
LMKD_TEXT = LMKD_TEXT.rstrip("\n") + "\n"
NETD="netd"
NETD_BINARY="system/bin/netd"
NETD_STOCK_RC="system/etc/init/netd.rc"
NETD_MANAGED_RC="scarlet/boot/services/netd.rc"
NETD_FIREWALL_BINARY="system/bin/iptables"
NETD_FIREWALL_ALIASES={"system/bin/iptables-restore":"iptables","system/bin/iptables-save":"iptables",
                       "system/bin/ip6tables":"iptables","system/bin/ip6tables-restore":"iptables",
                       "system/bin/ip6tables-save":"iptables"}
AUDIOSERVER="audioserver"
AUDIOSERVER_BINARY="system/bin/audioserver"
AUDIOSERVER_STOCK_RC="system/etc/init/audioserver.rc"
AUDIOSERVER_MANAGED_RC="scarlet/boot/services/audioserver.rc"
AUDIO_HAL="audio_hal"
AUDIO_HAL_BINARY="vendor/bin/hw/android.hardware.audio.service"
AUDIO_HAL_STOCK_RC="vendor/etc/init/android.hardware.audio.service.rc"
AUDIO_HAL_MANAGED_RC="scarlet/boot/services/audio_hal.rc"
AUDIO_START_BINARY="system/bin/scarlet-audio-late-start"
AUDIO_BINDER_COMPAT="vendor/lib64/libscarlet-audiobinder.so"
AUDIO_CONFIG_FILES=["vendor/etc/a2dp_in_audio_policy_configuration_7_0.xml","vendor/etc/audio_effects.xml",
                    "vendor/etc/audio_policy_configuration.xml","vendor/etc/audio_policy_volumes.xml",
                    "vendor/etc/bluetooth_audio_policy_configuration_7_0.xml",
                    "vendor/etc/default_volume_tables.xml",
                    "vendor/etc/primary_audio_policy_configuration.xml",
                    "vendor/etc/r_submix_audio_policy_configuration.xml"]
AUDIO_HAL_MODULES=["vendor/lib64/hw/android.hardware.audio@7.1-impl.ranchu.so",
                   "vendor/lib64/hw/android.hardware.audio.legacy@7.1-impl.ranchu.so",
                   "vendor/lib64/hw/android.hardware.audio.effect@7.0-impl.so",
                   "vendor/lib64/hw/audio.primary.default.so",
                   "vendor/lib64/hw/audio.r_submix.default.so",
                   "vendor/lib64/hw/audio.bluetooth.default.so",
                   "system/lib64/libaudiohal@7.1.so",
                   "system/lib64/libaudiohal_deathhandler.so"]
AUDIO_PRELOAD_MODULES=["vendor/lib64/hw/android.hardware.audio@7.1-impl.ranchu.so",
                       "vendor/lib64/hw/android.hardware.audio.legacy@7.1-impl.ranchu.so",
                       "vendor/lib64/hw/android.hardware.audio.effect@7.0-impl.so"]

def primary_audio_policy(data):
    lines=[];required={"primary_audio_policy_configuration.xml","audio_policy_volumes.xml","default_volume_tables.xml"};seen=set()
    for line in data.splitlines(keepends=True):
        match=re.search(rb'href="([^"]+)"',line)
        if match and match.group(1) in {b"a2dp_in_audio_policy_configuration_7_0.xml",b"r_submix_audio_policy_configuration.xml",b"bluetooth_audio_policy_configuration_7_0.xml"}:continue
        if match:seen.add(match.group(1).decode())
        lines.append(line)
    if not required.issubset(seen):raise ValueError("changed stock audio policy includes")
    return b"".join(lines)
VOLD_TEXT=("service vold /system/bin/vold \\\n"
           "        --blkid_context=u:r:blkid:s0 --blkid_untrusted_context=u:r:blkid_untrusted:s0 \\\n"
           "        --fsck_context=u:r:fsck:s0 --fsck_untrusted_context=u:r:fsck_untrusted:s0\n"
           "    class core\n    ioprio be 2\n    task_profiles ProcessCapacityHigh\n"
           "    shutdown critical\n    group root reserved_disk\n    reboot_on_failure reboot,vold-failed\n")
HEALTH_TEXT=("service health-hal-2-1 /"+HEALTH_BINARY+"\n"
             "    class hal charger\n    user system\n    group system\n"
             "    capabilities WAKE_ALARM BLOCK_SUSPEND\n    file /dev/kmsg w\n")
STOCK_TEXT = ("service system_suspend /" + BINARY + "\n"
              "    class early_hal\n    user system\n    group system wakelock\n"
              "    capabilities BLOCK_SUSPEND\n")


def validate_selection(profile):
    names = profile.get("host_services", [])
    if (not isinstance(names,list) or len(names)>12 or
            any(name not in (SUSPEND,HW_MANAGER,GRAPHICS,INSTALLER,ODSIGN,HEALTH,VOLD,LMKD,NETD,AUDIOSERVER,AUDIO_HAL,KEYSTORE) for name in names) or len(set(names))!=len(names)):
        raise ValueError("unsupported stock host service selection")
    selected = CONTRACT in profile.get("vintf", {}).get("framework", [])
    if selected != (SUSPEND in names):
        raise ValueError("SystemSuspend requires both its host provider and VINTF contract")
    from vintf_inputs import (graphics_selection, HEALTH_CONTRACT, AUDIO_CONTRACT, AUDIO_EFFECT_CONTRACT,
                              AUDIO_EFFECT_LEGACY_CONTRACT, FRAMEWORK_HIDL)
    if FRAMEWORK_HIDL.intersection(profile.get("vintf",{}).get("framework",[])) and HW_MANAGER not in names:
        raise ValueError("HIDL framework management requires its stock provider")
    device=profile.get("vintf",{}).get("device",[])
    graphics=[name for name in device if name not in
              (HEALTH_CONTRACT,AUDIO_CONTRACT,AUDIO_EFFECT_CONTRACT,AUDIO_EFFECT_LEGACY_CONTRACT)]
    if (GRAPHICS in names and (HW_MANAGER not in names or not graphics_selection(graphics)) or
            GRAPHICS not in names and graphics):raise ValueError("graphics services require their exact device inventory and hardware-service manager")
    if ((HEALTH in names)!=(HEALTH_CONTRACT in device) or
            HEALTH in names and HW_MANAGER not in names):
        raise ValueError("Health requires its exact device contract and hardware-service manager")
    audio_device = (AUDIO_CONTRACT in device and
                    (AUDIO_EFFECT_CONTRACT in device or AUDIO_EFFECT_LEGACY_CONTRACT in device))
    if ((AUDIO_HAL in names)!=audio_device or
            AUDIO_HAL in names and HW_MANAGER not in names):
        raise ValueError("Audio HAL requires its exact device contract and hardware-service manager")
    return names


def imports(profile):
    return ["/"+{SUSPEND:MANAGED_RC,HW_MANAGER:HW_MANAGED_RC,GRAPHICS:GRAPHICS_RC,INSTALLER:INSTALLER_MANAGED_RC,ODSIGN:ODSIGN_MANAGED_RC,HEALTH:HEALTH_MANAGED_RC,VOLD:VOLD_MANAGED_RC,LMKD:LMKD_MANAGED_RC,NETD:NETD_MANAGED_RC,AUDIOSERVER:AUDIOSERVER_MANAGED_RC,AUDIO_HAL:AUDIO_HAL_MANAGED_RC,KEYSTORE:KEYSTORE_MANAGED_RC}[name] for name in validate_selection(profile)]


def build_keystore(stock_rc,binary):
    directives = [line.strip() for line in stock_rc.decode().splitlines()
                  if line.strip() and not line.lstrip().startswith("#")]
    if directives != ["service keystore2 /system/bin/keystore2 /data/misc/keystore",
                      "class early_hal", "user keystore", "group keystore readproc log",
                      "task_profiles ProcessCapacityHigh"]:
        raise ValueError("changed stock keystore2 service configuration")
    digest=lambda data:hashlib.sha256(data).hexdigest()
    return stock_rc,{"source_rc":KEYSTORE_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                     "binary_sha256":digest(binary),"managed_rc_sha256":digest(stock_rc)}


def build_vold(stock_rc,binary):
    if stock_rc.decode()!=VOLD_TEXT:raise ValueError("changed stock vold service configuration")
    managed=stock_rc.replace(b"    ioprio be 2\n",b"").replace(b"    reboot_on_failure reboot,vold-failed\n",b"")
    digest=lambda data:hashlib.sha256(data).hexdigest()
    return managed,{"source_rc":VOLD_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                    "binary_sha256":digest(binary),"managed_rc_sha256":digest(managed),
                    "outer_volume_policy":True,"host_reboot":False,"omitted_ioprio":"be 2"}


def build_lmkd(stock_rc,binary):
    if not stock_rc.startswith(b"service lmkd /system/bin/lmkd\n") or b"    socket lmkd seqpacket+passcred 0660 system system\n" not in stock_rc:
        raise ValueError("changed stock lmkd service configuration")
    omitted="    capabilities DAC_OVERRIDE KILL IPC_LOCK SYS_NICE SYS_RESOURCE\n".encode()
    if stock_rc.count(omitted)!=1:raise ValueError("changed lmkd capability declaration")
    managed=stock_rc.replace(omitted,b"")
    digest=lambda data:hashlib.sha256(data).hexdigest()
    return managed,{"source_rc":LMKD_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                    "binary_sha256":digest(binary),"managed_rc_sha256":digest(managed),
                    "omitted_capabilities":["DAC_OVERRIDE","KILL","IPC_LOCK","SYS_NICE","SYS_RESOURCE"],
                    "socket":{"name":"lmkd","type":"seqpacket","credentials":True,"mode":"0660"}}


def build_netd(stock_rc,binary):
    if not stock_rc.startswith(b"service netd /system/bin/netd\n") or b"    socket dnsproxyd stream 0660 root inet\n" not in stock_rc:
        raise ValueError("changed stock netd service configuration")
    capabilities=b"    capabilities CHOWN DAC_OVERRIDE DAC_READ_SEARCH FOWNER IPC_LOCK KILL NET_ADMIN NET_BIND_SERVICE NET_RAW SETUID SETGID\n"
    if stock_rc.count(capabilities)!=1:raise ValueError("changed netd capability declaration")
    marker=b"service netd /system/bin/netd\n"
    restart=b"    onrestart restart zygote\n    onrestart restart zygote_secondary\n"
    if stock_rc.count(restart)!=1:raise ValueError("changed netd restart policy")
    managed=(stock_rc.replace(marker,b"# eBPF loading is not attached to this boot profile.\n"
        b"on init\n    setprop bpf.progs_loaded 1\n\n"+marker)
        .replace(capabilities,b"")
        .replace(restart,b"# A failed optional network daemon must not restart Zygote.\n    oneshot\n    disabled\n"))
    digest=lambda data:hashlib.sha256(data).hexdigest()
    return managed,{"source_rc":NETD_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                    "binary_sha256":digest(binary),"managed_rc_sha256":digest(managed),
                    "firewall_binary":NETD_FIREWALL_BINARY,"firewall_aliases":NETD_FIREWALL_ALIASES,
                    "firewall_implementation":"scarlet-compat-v1","packet_filtering":"unavailable",
                    "bpf_loader":"unavailable",
                    "omitted_capabilities":["CHOWN","DAC_OVERRIDE","DAC_READ_SEARCH","FOWNER","IPC_LOCK","KILL","NET_ADMIN","NET_BIND_SERVICE","NET_RAW","SETUID","SETGID"]}


def build_audioserver(stock_rc,binary):
    if b"service audioserver /system/bin/audioserver\n" not in stock_rc or b"    user audioserver\n" not in stock_rc:
        raise ValueError("changed stock audioserver configuration")
    managed=stock_rc
    for line in (b"    capabilities BLOCK_SUSPEND\n",b"    ioprio rt 4\n",b"    task_profiles ProcessCapacityHigh HighPerformance\n"):
        if managed.count(line)!=1:raise ValueError("changed audioserver scheduling declaration")
        managed=managed.replace(line,b"")
    lines=[];running=False;inserted=False
    for line in managed.splitlines():
        if line.startswith(b"on property:init.svc.audioserver=running"):running=True
        elif line.startswith(b"on "):running=False
        if line.strip().startswith(b"start vendor.audio-hal"):
            if running and not inserted:
                lines.append(b"    start audio-start-helper")
                inserted=True
            continue
        if not line.startswith(b"    onrestart "):lines.append(line)
    if not inserted:raise ValueError("missing audioserver readiness action")
    managed=(b"\n".join(lines)+b"\n\nservice audio-start-helper /"+AUDIO_START_BINARY.encode()+
             b"\n    class core\n    user root\n    group root\n    seclabel u:r:init:s0\n    oneshot\n    disabled\n")
    digest=lambda data:hashlib.sha256(data).hexdigest()
    return managed,{"source_rc":AUDIOSERVER_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                    "binary_sha256":digest(binary),"managed_rc_sha256":digest(managed),
                    "rt_scheduling":"unavailable","start_helper":AUDIO_START_BINARY}


def build_audio_hal(stock_rc,binary):
    if b"service vendor.audio-hal /vendor/bin/hw/android.hardware.audio.service\n" not in stock_rc:
        raise ValueError("changed stock audio HAL configuration")
    managed=stock_rc
    for line in (b"    capabilities BLOCK_SUSPEND\n",b"    ioprio rt 4\n",b"    task_profiles ProcessCapacityHigh HighPerformance\n"):
        if managed.count(line)!=1:raise ValueError("changed audio HAL scheduling declaration")
        managed=managed.replace(line,b"")
    managed=managed.replace(b"    class hal\n",b"    class hal\n    disabled\n")
    preload=b"    setenv LD_PRELOAD /"+AUDIO_BINDER_COMPAT.encode()+b"".join(
        b":/"+path.encode() for path in AUDIO_PRELOAD_MODULES)+b"\n"
    managed=managed.replace(b"    group audio camera drmrpc inet media mediadrm net_bt net_bt_admin net_bw_acct wakelock context_hub\n",
        b"    group audio camera drmrpc inet media mediadrm net_bt net_bt_admin net_bw_acct wakelock context_hub\n"
        b"    chdir /\n    setenv LD_LIBRARY_PATH /vendor/lib64/hw:/vendor/lib64:/system/lib64\n"+preload)
    managed=b"\n".join(line for line in managed.splitlines() if not line.startswith(b"    onrestart "))+b"\n"
    digest=lambda data:hashlib.sha256(data).hexdigest()
    return managed,{"source_rc":AUDIO_HAL_STOCK_RC,"source_rc_sha256":digest(stock_rc),
                    "binary_sha256":digest(binary),"managed_rc_sha256":digest(managed),
                    "rt_scheduling":"unavailable","audio_output":"stock HAL initialization only",
                    "configuration_files":AUDIO_CONFIG_FILES,"start_helper":AUDIO_START_BINARY,
                    "binder_compat":AUDIO_BINDER_COMPAT,"audio_modules":["primary"]}


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
    if VOLD in names:
        record=proof[VOLD]
        if (metadata["files"].get(VOLD_BINARY)!=record.get("binary_sha256") or
                metadata["files"].get(VOLD_MANAGED_RC)!=record.get("managed_rc_sha256") or
                record.get("outer_volume_policy") is not True or record.get("host_reboot") is not False or
                record.get("omitted_ioprio")!="be 2"):
            raise ValueError("changed vold provider or outer management boundary")
        files=record.get("files",{})
        if (files.get(VOLD_BINARY)!=record.get("binary_sha256") or
                set(record.get("native_needed",{}))!=set(files) or
                any(metadata["files"].get(path)!=digest for path,digest in files.items())):
            raise ValueError("changed vold dependency inputs")
    if LMKD in names:
        record=proof[LMKD]
        if (metadata["files"].get(LMKD_BINARY)!=record.get("binary_sha256") or
                metadata["files"].get(LMKD_MANAGED_RC)!=record.get("managed_rc_sha256") or
                record.get("omitted_capabilities") != ["DAC_OVERRIDE","KILL","IPC_LOCK","SYS_NICE","SYS_RESOURCE"]):
            raise ValueError("changed lmkd provider inputs")
        files=record.get("files",{})
        if (files.get(LMKD_BINARY)!=record.get("binary_sha256") or
                set(record.get("native_needed",{}))!=set(files) or
                any(metadata["files"].get(path)!=digest for path,digest in files.items())):
            raise ValueError("changed lmkd dependency inputs")
    if NETD in names:
        record=proof[NETD]
        if (metadata["files"].get(NETD_BINARY)!=record.get("binary_sha256") or
                metadata["files"].get(NETD_MANAGED_RC)!=record.get("managed_rc_sha256") or
                record.get("bpf_loader")!="unavailable" or
                record.get("firewall_binary")!=NETD_FIREWALL_BINARY or
                record.get("firewall_aliases")!=NETD_FIREWALL_ALIASES or
                record.get("firewall_implementation")!="scarlet-compat-v1" or
                record.get("packet_filtering")!="unavailable" or
                not record.get("omitted_capabilities")):
            raise ValueError("changed netd provider inputs")
        files=record.get("files",{})
        if (files.get(NETD_BINARY)!=record.get("binary_sha256") or
                set(record.get("native_needed",{}))!=set(files) or
                any(metadata["files"].get(path)!=digest for path,digest in files.items())):
            raise ValueError("changed netd dependency inputs")
    for name,binary,managed_rc in ((AUDIOSERVER,AUDIOSERVER_BINARY,AUDIOSERVER_MANAGED_RC),
                                   (AUDIO_HAL,AUDIO_HAL_BINARY,AUDIO_HAL_MANAGED_RC)):
        if name not in names:continue
        record=proof[name];files=record.get("files",{})
        if (metadata["files"].get(binary)!=record.get("binary_sha256") or
                metadata["files"].get(managed_rc)!=record.get("managed_rc_sha256") or
                record.get("rt_scheduling")!="unavailable" or
                record.get("start_helper")!=AUDIO_START_BINARY or
                name==AUDIO_HAL and record.get("configuration_files")!=AUDIO_CONFIG_FILES or
                name==AUDIO_HAL and record.get("binder_compat")!=AUDIO_BINDER_COMPAT or
                name==AUDIO_HAL and record.get("audio_modules")!= ["primary"] or
                files.get(binary)!=record.get("binary_sha256") or
                set(record.get("native_needed",{}))!=set(files) or
                any(metadata["files"].get(path)!=digest for path,digest in files.items())):
            raise ValueError("changed audio provider inputs")
    for name,binary,rc in [(INSTALLER,INSTALLER_BINARY,INSTALLER_MANAGED_RC),(ODSIGN,ODSIGN_BINARY,ODSIGN_MANAGED_RC),(KEYSTORE,KEYSTORE_BINARY,KEYSTORE_MANAGED_RC)]:
        if name not in names:continue
        record=proof[name]
        if (metadata["files"].get(binary)!=record.get("binary_sha256") or
                metadata["files"].get(rc)!=record.get("managed_rc_sha256") or
                record.get("source_rc_sha256")!=record.get("managed_rc_sha256")):
            raise ValueError("changed stock service inputs: "+name)
