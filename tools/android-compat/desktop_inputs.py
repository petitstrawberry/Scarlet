"""Pinned stock packages selected by the Android manager's home profile."""
from art_inputs import sha256

PACKAGES = {
    "system": ["system/app/ExtShared", "system/priv-app/SettingsProvider", "system/priv-app/PackageInstaller",
               "system/priv-app/FusedLocation", "system/priv-app/IntentResolver",
               "system/priv-app/Shell", "system/priv-app/InputDevices"],
    "system_ext": ["priv-app/Launcher3QuickStep", "priv-app/SystemUI", "priv-app/Provision", "priv-app/SdkSetup"],
    "product": ["app/LatinIME", "app/ModuleMetadata"],
}
CONFIGURATION = {
    "system": ["system/etc/selinux/plat_mac_permissions.xml",
               "system/etc/permissions/com.android.intentresolver.xml",
               "system/etc/permissions/platform.xml", "system/etc/permissions/privapp-permissions-platform.xml",
               "system/etc/sysconfig/framework-sysconfig.xml", "system/etc/sysconfig/preinstalled-packages-platform.xml"],
    "system_ext": ["etc/permissions/" + name + ".xml" for name in
                   ["com.android.launcher3", "com.android.systemui", "com.android.provision", "com.android.sdksetup",
                    "androidx.window.extensions", "androidx.window.sidecar"]] +
                  ["framework/androidx.window.extensions.jar", "framework/androidx.window.sidecar.jar"],
    "vendor": ["etc/selinux/vendor_mac_permissions.xml"],
}


def validate_selection(profile):
    if "package_set" in profile and profile["package_set"] != "stock-home":
        raise ValueError("unsupported Android package selection")


def prepare(profile, images, temporary, entries):
    validate_selection(profile)
    if "package_set" not in profile:
        return {}, None
    files, packages = {}, {}
    def install(partition, original):
        relative = original if partition == "system" else partition + "/" + original
        target = temporary / "files" / relative
        images[partition].dump("/" + original, target)
        files[relative] = target
        return relative
    for partition, paths in PACKAGES.items():
        for directory in paths:
            apks = [name for name, mode, _ in entries(images[partition], "/" + directory)
                    if name.endswith(".apk") and mode & 0o170000 == 0o100000]
            if not apks or len(apks) > 16:
                raise ValueError("missing or excessive stock package payloads")
            packages[partition + ":/" + directory] = [install(partition, directory + "/" + name) for name in apks]
    for partition, names in CONFIGURATION.items():
        for name in names:
            install(partition, name)
    return files, {"selection": "stock-home", "packages": packages,
                   "files": {name: sha256(path) for name, path in files.items()}}


def validate_staged(metadata):
    selected = metadata["profile"].get("package_set")
    proof = metadata.get("desktop_inputs")
    if not selected:
        if proof:
            raise ValueError("unselected desktop inputs")
        return
    if not proof or proof.get("selection") != selected:
        raise ValueError("missing stock home package inputs")
    expected = {partition + ":/" + name for partition, names in PACKAGES.items() for name in names}
    if set(proof.get("packages", {})) != expected:
        raise ValueError("incomplete home package selection")
    configuration = {name if partition == "system" else partition + "/" + name
                     for partition, names in CONFIGURATION.items() for name in names}
    payloads = set()
    for group, names in proof["packages"].items():
        partition, directory = group.split(":/", 1)
        prefix = directory if partition == "system" else partition + "/" + directory
        if not 0 < len(names) <= 16 or len(set(names)) != len(names):
            raise ValueError("invalid stock package payload count")
        for name in names:
            if not name.startswith(prefix + "/") or "/" in name[len(prefix)+1:] or not name.endswith(".apk"):
                raise ValueError("unexpected package destination")
        payloads.update(names)
    if set(proof.get("files", {})) != configuration | payloads:
        raise ValueError("incomplete stock home configuration")
    for name, digest in proof["files"].items():
        if metadata["files"].get(name) != digest:
            raise ValueError("changed stock home input")
    for names in proof["packages"].values():
        if not names or any(name not in proof["files"] for name in names):
            raise ValueError("unrecorded stock package")
