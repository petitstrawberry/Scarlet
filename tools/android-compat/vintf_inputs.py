"""Select declared framework AIDL contracts from verified stock XML inputs."""
import hashlib
import re
import xml.etree.ElementTree as ET

FRAMEWORK = "system/etc/vintf/manifest.xml"
DEVICE = "vendor/etc/vintf/manifest.xml"
SERVICE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+/[A-Za-z0-9_.-]+")
GRAPHICS_DEVICE = {
    "android.hardware.graphics.allocator@2.0::IAllocator/default": "hwbinder",
    "android.hardware.graphics.mapper@2.0::IMapper/default": "passthrough",
    "android.hardware.graphics.composer@2.1::IComposer/default": "hwbinder",
    "android.hardware.graphics.composer@2.4::IComposer/default": "hwbinder",
}
KEYMINT_DEVICE = {
    "android.hardware.security.keymint.IKeyMintDevice/default":
        ("android.hardware.security.keymint", "2", "IKeyMintDevice/default"),
    "android.hardware.security.keymint.IRemotelyProvisionedComponent/default":
        ("android.hardware.security.keymint", "2", "IRemotelyProvisionedComponent/default"),
    "android.hardware.security.secureclock.ISecureClock/default":
        ("android.hardware.security.secureclock", "1", "ISecureClock/default"),
    "android.hardware.security.sharedsecret.ISharedSecret/default":
        ("android.hardware.security.sharedsecret", "1", "ISharedSecret/default"),
}
HEALTH_CONTRACT = "android.hardware.health@2.1::IHealth/default"
AUDIO_CONTRACT = "android.hardware.audio@7.1::IDevicesFactory/default"
AUDIO_EFFECT_CONTRACT = "android.hardware.audio.effect@7.0::IEffectsFactory/default"
AUDIO_EFFECT_LEGACY_CONTRACT = "android.hardware.audio.effect@7.1::IEffectsFactory/default"
GATEKEEPER_CONTRACT = "android.hardware.gatekeeper@1.0::IGatekeeper/default"
DEVICE_PROVIDERS = {**GRAPHICS_DEVICE, HEALTH_CONTRACT: "hwbinder", AUDIO_CONTRACT: "hwbinder",
                    AUDIO_EFFECT_CONTRACT: "hwbinder", AUDIO_EFFECT_LEGACY_CONTRACT: "hwbinder",
                    GATEKEEPER_CONTRACT: "hwbinder",
                    **{contract: "aidl" for contract in KEYMINT_DEVICE}}
FRAMEWORK_HIDL = {
    "android.hidl.manager@1.2::IServiceManager/default",
    "android.hidl.token@1.0::ITokenManager/default",
    "android.frameworks.sensorservice@1.0::ISensorManager/default",
    "android.frameworks.schedulerservice@1.0::ISchedulingPolicyService/default",
    "android.system.net.netd@1.1::INetd/default",
}


def graphics_selection(device):
    base={"android.hardware.graphics.allocator@2.0::IAllocator/default","android.hardware.graphics.mapper@2.0::IMapper/default"}
    return any(set(device)==base|{f"android.hardware.graphics.composer@{version}::IComposer/default"} for version in ["2.1","2.4"])


def validate_selection(selection):
    if not isinstance(selection, dict) or set(selection) != {"framework", "device"}:
        raise ValueError("invalid VINTF selection")
    names = selection["framework"]
    if (not isinstance(names, list) or len(names) > 64 or
            any(not isinstance(name, str) or len(name) > 256 or
                not (SERVICE.fullmatch(name) or name in FRAMEWORK_HIDL) for name in names) or
            len(set(names)) != len(names)):
        raise ValueError("invalid framework VINTF service selection")
    device=selection["device"]
    if (not isinstance(device,list) or len(device)>len(DEVICE_PROVIDERS) or
            any(not isinstance(name,str) or name not in DEVICE_PROVIDERS for name in device) or len(set(device))!=len(device)):
        raise ValueError("device HAL declarations require a configured Scarlet provider")
    return selection


def build_manifests(selection, sources, device_providers=None):
    validate_selection(selection)
    providers=device_providers or {}
    if set(providers)!=set(selection["device"]):raise ValueError("device HAL selection requires exact provider inputs")
    candidates = {}
    for name, data in sources.items():
        if len(data) > 256 * 1024:
            raise ValueError("VINTF source exceeds input bound")
        root = ET.fromstring(data)
        if root.tag != "manifest" or root.get("type") != "framework":
            continue
        for hal in root.findall("hal"):
            kind=hal.get("format")
            if kind not in ("aidl","hidl"):
                continue
            package = hal.findtext("name", "")
            versions = [value.text for value in hal.findall("version")] or ["1"]
            instances = [value.text for value in hal.findall("fqname")]
            for interface in hal.findall("interface"):
                instances.extend(f"{interface.findtext('name')}/{value.text}"
                                 for value in interface.findall("instance"))
            declared=set()
            for instance in dict.fromkeys(instances):
                if kind=="hidl" and not isinstance(instance,str):raise ValueError("invalid HIDL instance")
                if kind=="hidl" and not instance.startswith("@"):
                    if len(versions)!=1:raise ValueError("ambiguous HIDL interface version")
                    instance="@"+versions[0]+"::"+instance
                service = f"{package}.{instance}" if kind=="aidl" else package+instance
                if service not in selection["framework"]:
                    continue
                if service in declared:continue
                declared.add(service)
                if kind=="hidl":
                    version=instance[1:].split("::",1)[0]
                    if service not in FRAMEWORK_HIDL or hal.findtext("transport")!="hwbinder":
                        raise ValueError("unsupported framework HIDL contract")
                elif len(versions) != 1 or not isinstance(versions[0], str) or not re.fullmatch(r"[1-9][0-9]{0,4}", versions[0]):
                    raise ValueError("ambiguous or unsupported AIDL VINTF version")
                else:version=versions[0]
                record = {"source": name, "source_sha256": hashlib.sha256(data).hexdigest(),
                          "package": package, "version": version, "fqname": instance}
                if kind=="hidl":record.update(format=kind,transport="hwbinder")
                if service in candidates:
                    raise ValueError("duplicate framework VINTF service declaration")
                candidates[service] = record
    framework = ET.Element("manifest", {"version": "5.0", "type": "framework"})
    for service in selection["framework"]:
        if service not in candidates:
            raise ValueError("selected VINTF service absent from stock input: " + service)
        record = candidates[service]
        kind=record.get("format","aidl")
        hal = ET.SubElement(framework, "hal", {"format": kind})
        if kind=="hidl":ET.SubElement(hal,"transport").text=record["transport"]
        for tag in ("name", "version", "fqname"):
            ET.SubElement(hal, tag).text = record["package" if tag == "name" else tag]
    device = ET.Element("manifest", {"version": "5.0", "type": "device"})
    for contract in selection["device"]:
        if providers[contract].get("transport")!=DEVICE_PROVIDERS[contract]:raise ValueError("device provider transport mismatch")
        if contract in KEYMINT_DEVICE:
            package, version, fqname = KEYMINT_DEVICE[contract]
            hal=ET.SubElement(device,"hal",{"format":"aidl"})
            ET.SubElement(hal,"name").text=package
            ET.SubElement(hal,"version").text=version
            ET.SubElement(hal,"fqname").text=fqname
            continue
        package,fqname=contract.split("@",1)
        hal=ET.SubElement(device,"hal",{"format":"hidl"})
        ET.SubElement(hal,"name").text=package
        attributes={"arch":"64"} if DEVICE_PROVIDERS[contract]=="passthrough" else {}
        ET.SubElement(hal,"transport",attributes).text=DEVICE_PROVIDERS[contract]
        ET.SubElement(hal,"fqname").text="@"+fqname
    outputs = {}
    for name, root in [(FRAMEWORK, framework), (DEVICE, device)]:
        ET.indent(root)
        outputs[name] = ET.tostring(root, encoding="utf-8", xml_declaration=True) + b"\n"
    proof = {"selection": selection, "contracts": candidates,
             "generated_sha256": {name: hashlib.sha256(data).hexdigest() for name, data in outputs.items()}}
    if providers:proof["device_providers"]=providers
    return outputs, proof


def validate_staged(metadata):
    selection = metadata["profile"].get("vintf")
    if selection is None:
        return
    proof = metadata.get("vintf_inputs", {})
    if (proof.get("selection") != selection or set(proof.get("contracts", {})) != set(selection["framework"]) or
            set(proof.get("generated_sha256", {})) != {FRAMEWORK, DEVICE} or
            any(metadata["files"].get(name) != digest for name, digest in proof["generated_sha256"].items())):
        raise ValueError("profile requires matching verified VINTF inputs; prepare with --platform-image")
    providers=proof.get("device_providers",{})
    if set(providers)!=set(selection["device"]):raise ValueError("device inventory lacks verified providers")
    for name,provider in providers.items():
        if (provider.get("transport")!=DEVICE_PROVIDERS[name] or not provider.get("path") or
                metadata["files"].get(provider["path"])!=provider.get("sha256")):
            raise ValueError("device inventory provider changed")
    for name in set(metadata["files"]) | set(metadata.get("symlinks", {})):
        manifest = name.endswith("/etc/vintf/manifest.xml") or "/etc/vintf/manifest/" in name
        legacy = name in {"system/manifest.xml", "vendor/manifest.xml", "odm/manifest.xml"}
        if (manifest or legacy) and name not in {FRAMEWORK, DEVICE}:
            raise ValueError("unselected VINTF inventory in boot inputs: " + name)
