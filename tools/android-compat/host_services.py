"""Explicit stock services whose Android instance does not control host power."""
import hashlib

SUSPEND = "system_suspend"
BINARY = "system/bin/hw/android.system.suspend@1.0-service"
STOCK_RC = "system/etc/init/android.system.suspend@1.0-service.rc"
MANAGED_RC = "scarlet/boot/services/system_suspend.rc"
CONTRACT = "android.system.suspend.ISystemSuspend/default"
STOCK_TEXT = ("service system_suspend /" + BINARY + "\n"
              "    class early_hal\n    user system\n    group system wakelock\n"
              "    capabilities BLOCK_SUSPEND\n")


def validate_selection(profile):
    names = profile.get("host_services", [])
    if names not in ([], [SUSPEND]):
        raise ValueError("unsupported stock host service selection")
    selected = CONTRACT in profile.get("vintf", {}).get("framework", [])
    if selected != (SUSPEND in names):
        raise ValueError("SystemSuspend requires both its host provider and VINTF contract")
    return names


def imports(profile):
    return ["/" + MANAGED_RC] if SUSPEND in validate_selection(profile) else []


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
