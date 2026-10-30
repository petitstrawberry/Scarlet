"""Select declared framework AIDL contracts from verified stock XML inputs."""
import hashlib
import re
import xml.etree.ElementTree as ET

FRAMEWORK = "system/etc/vintf/manifest.xml"
DEVICE = "vendor/etc/vintf/manifest.xml"
SERVICE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+/[A-Za-z0-9_.-]+")


def validate_selection(selection):
    if not isinstance(selection, dict) or set(selection) != {"framework", "device"}:
        raise ValueError("invalid VINTF selection")
    names = selection["framework"]
    if (not isinstance(names, list) or len(names) > 64 or
            any(not isinstance(name, str) or len(name) > 256 or not SERVICE.fullmatch(name) for name in names) or
            len(set(names)) != len(names)):
        raise ValueError("invalid framework VINTF service selection")
    # No Scarlet device HAL provider has been configured by this bring-up path.
    if selection["device"] != []:
        raise ValueError("device HAL declarations require a configured Scarlet provider")
    return selection


def build_manifests(selection, sources):
    validate_selection(selection)
    candidates = {}
    for name, data in sources.items():
        if len(data) > 256 * 1024:
            raise ValueError("VINTF source exceeds input bound")
        root = ET.fromstring(data)
        if root.tag != "manifest" or root.get("type") != "framework":
            continue
        for hal in root.findall("hal"):
            if hal.get("format") != "aidl":
                continue
            package = hal.findtext("name", "")
            versions = [value.text for value in hal.findall("version")] or ["1"]
            instances = [value.text for value in hal.findall("fqname")]
            for interface in hal.findall("interface"):
                instances.extend(f"{interface.findtext('name')}/{value.text}"
                                 for value in interface.findall("instance"))
            for instance in instances:
                service = f"{package}.{instance}"
                if service not in selection["framework"]:
                    continue
                if len(versions) != 1 or not isinstance(versions[0], str) or not re.fullmatch(r"[1-9][0-9]{0,4}", versions[0]):
                    raise ValueError("ambiguous or unsupported AIDL VINTF version")
                record = {"source": name, "source_sha256": hashlib.sha256(data).hexdigest(),
                          "package": package, "version": versions[0], "fqname": instance}
                if service in candidates:
                    raise ValueError("duplicate framework VINTF service declaration")
                candidates[service] = record
    framework = ET.Element("manifest", {"version": "5.0", "type": "framework"})
    for service in selection["framework"]:
        if service not in candidates:
            raise ValueError("selected VINTF service absent from stock input: " + service)
        record = candidates[service]
        hal = ET.SubElement(framework, "hal", {"format": "aidl"})
        for tag in ("name", "version", "fqname"):
            ET.SubElement(hal, tag).text = record["package" if tag == "name" else tag]
    device = ET.Element("manifest", {"version": "5.0", "type": "device"})
    outputs = {}
    for name, root in [(FRAMEWORK, framework), (DEVICE, device)]:
        ET.indent(root)
        outputs[name] = ET.tostring(root, encoding="utf-8", xml_declaration=True) + b"\n"
    proof = {"selection": selection, "contracts": candidates,
             "generated_sha256": {name: hashlib.sha256(data).hexdigest() for name, data in outputs.items()}}
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
    for name in set(metadata["files"]) | set(metadata.get("symlinks", {})):
        manifest = name.endswith("/etc/vintf/manifest.xml") or "/etc/vintf/manifest/" in name
        legacy = name in {"system/manifest.xml", "vendor/manifest.xml", "odm/manifest.xml"}
        if (manifest or legacy) and name not in {FRAMEWORK, DEVICE}:
            raise ValueError("unselected VINTF inventory in boot inputs: " + name)
