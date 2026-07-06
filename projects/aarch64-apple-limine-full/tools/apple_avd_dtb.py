#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Patch Apple AVD nodes into m1n1 guest DTBs.

The U-Boot payload DTB carried by the local m1n1 tree does not currently
include AVD or dart-avd nodes. Scarlet can only probe the Apple AVD driver when
those nodes are present in the guest DTB, so this helper extracts the real
addresses from m1n1's live ADT and applies a small DT overlay to the guest
payload before it is loaded by the hypervisor.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any


FDT_MAGIC = b"\xd0\r\xfe\xed"
AVD_ADT_PATH = "/arm-io/avd"
DART_AVD_ADT_PATH = "/arm-io/dart-avd"
PMGR_ADT_PATH = "/arm-io/pmgr"
GUEST_SOC_PATH = "/soc"
PMGR_DEVICE_ID_MASK = 0xffff
PMGR_DIE_ID_SHIFT = 28
PMGR_DIE_ID_MASK = 0xf
PMGR_DIE_OFFSET = 0x2000000000


class AvdDtbError(RuntimeError):
    """Raised when an AVD DTB patch cannot be produced."""


def _run(cmd: list[str | pathlib.Path], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(part) for part in cmd],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **kwargs,
    )


def _tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise AvdDtbError(f"{name} not found; install device-tree-compiler")
    return path


def _node_by_path(adt: Any, path: str) -> Any:
    try:
        return adt[path]
    except Exception as exc:
        raise AvdDtbError(f"live ADT is missing {path}") from exc


def _node_compatibles(node: Any) -> list[str]:
    value = node.getprop("compatible", None) if hasattr(node, "getprop") else None
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    if isinstance(value, bytes):
        return [part.decode("ascii") for part in value.rstrip(b"\0").split(b"\0") if part]
    try:
        return [str(item) for item in value]
    except TypeError:
        return [str(value)]


def _u32_list(value: Any, label: str) -> list[int]:
    if value is None:
        return []
    if isinstance(value, int):
        values = [value]
    elif isinstance(value, bytes):
        if len(value) % 4 != 0:
            raise AvdDtbError(f"{label} byte length is not a multiple of 4")
        values = [
            int.from_bytes(value[index : index + 4], "little")
            for index in range(0, len(value), 4)
        ]
    else:
        try:
            values = [int(item) for item in value]
        except TypeError as exc:
            raise AvdDtbError(f"{label} is not a u32 list") from exc
    for item in values:
        if item < 0 or item > 0xffffffff:
            raise AvdDtbError(f"{label} contains out-of-range u32 value {item:#x}")
    return values


def _node_u32_list(node: Any, prop: str, path: str) -> list[int]:
    value = node.getprop(prop, None) if hasattr(node, "getprop") else None
    return _u32_list(value, f"{path}.{prop}")


def _field(value: Any, name: str) -> Any:
    if isinstance(value, dict):
        return value[name]
    return getattr(value, name)


def _maybe_field(value: Any, name: str, default: Any = None) -> Any:
    try:
        return _field(value, name)
    except (AttributeError, KeyError):
        return default


def _pmgr_uses_u8_ids(devices: list[Any]) -> bool:
    if len(devices) < 2:
        return False
    return int(_field(devices[0], "id1")) != int(_field(devices[1], "id1"))


def _pmgr_device_id(device: Any, use_u8_ids: bool) -> int:
    return int(_field(device, "id1" if use_u8_ids else "id2"))


def _pmgr_device_parents(device: Any, use_u8_ids: bool) -> list[int]:
    parents_un = _field(device, "parents_un")
    field = "u8id" if use_u8_ids else "u16id"
    parents = _field(_field(parents_un, field), "parents")
    return [int(parent) for parent in parents if int(parent) != 0]


def _pmgr_device_is_virtual(device: Any) -> bool:
    flags = _field(device, "flags")
    if isinstance(flags, int):
        return bool(flags & 0x10)
    # m1n1's Python ADT parser names the PMGR_FLAG_VIRTUAL bit `no_ps`.
    return bool(_maybe_field(flags, "no_ps", False))


def _find_pmgr_device(devices: list[Any], use_u8_ids: bool, device_id: int) -> Any | None:
    for device in devices:
        if _pmgr_device_id(device, use_u8_ids) == device_id:
            return device
    return None


def _pmgr_device_paddr(pmgr: Any, ps_regs: list[Any], device: Any, die: int) -> int:
    psreg_index = int(_field(device, "psreg"))
    psreg = ps_regs[psreg_index]
    reg_index = int(_field(psreg, "reg"))
    reg_offset = int(_field(psreg, "offset"))
    base, _size = pmgr.get_reg(reg_index)
    addr_offset = int(_field(device, "psidx")) << 3
    return int(base) + reg_offset + (PMGR_DIE_OFFSET * die) + addr_offset


def _pmgr_clock_gate_record(
    pmgr: Any,
    ps_regs: list[Any],
    device: Any,
    use_u8_ids: bool,
    gate: int,
    die: int,
) -> dict[str, Any]:
    is_virtual = _pmgr_device_is_virtual(device)
    paddr = None
    if not is_virtual:
        try:
            paddr = _pmgr_device_paddr(pmgr, ps_regs, device, die)
        except Exception:
            paddr = None
    return {
        "gate": gate,
        "die": die,
        "device_id": _pmgr_device_id(device, use_u8_ids),
        "name": str(_field(device, "name")),
        "parents": _pmgr_device_parents(device, use_u8_ids),
        "virtual": is_virtual,
        "paddr": paddr,
    }


def _collect_pmgr_clock_gate_device(
    pmgr: Any,
    devices: list[Any],
    ps_regs: list[Any],
    use_u8_ids: bool,
    gate: int,
    die: int,
    device_id: int,
    out: list[dict[str, Any]],
    seen: set[tuple[int, int]],
    stack: set[tuple[int, int]],
) -> None:
    if device_id == 0:
        return
    key = (die, device_id)
    if key in stack:
        return
    device = _find_pmgr_device(devices, use_u8_ids, device_id)
    if device is None:
        return

    stack.add(key)
    is_virtual = _pmgr_device_is_virtual(device)
    if is_virtual and key not in seen:
        out.append(
            _pmgr_clock_gate_record(pmgr, ps_regs, device, use_u8_ids, gate, die)
        )
        seen.add(key)

    for parent_id in _pmgr_device_parents(device, use_u8_ids):
        _collect_pmgr_clock_gate_device(
            pmgr,
            devices,
            ps_regs,
            use_u8_ids,
            gate,
            die,
            parent_id,
            out,
            seen,
            stack,
        )

    if not is_virtual and key not in seen:
        out.append(
            _pmgr_clock_gate_record(pmgr, ps_regs, device, use_u8_ids, gate, die)
        )
        seen.add(key)
    stack.remove(key)


def _pmgr_clock_gate_devices(adt: Any, gates: list[int]) -> list[dict[str, Any]]:
    try:
        pmgr = _node_by_path(adt, PMGR_ADT_PATH)
        devices = list(pmgr.getprop("devices", []))
        ps_regs = list(pmgr.getprop("ps-regs", []))
    except Exception:
        return []
    if not devices or not ps_regs:
        return []

    use_u8_ids = _pmgr_uses_u8_ids(devices)
    out: list[dict[str, Any]] = []
    seen: set[tuple[int, int]] = set()
    for gate in gates:
        device_id = gate & PMGR_DEVICE_ID_MASK
        die = (gate >> PMGR_DIE_ID_SHIFT) & PMGR_DIE_ID_MASK
        _collect_pmgr_clock_gate_device(
            pmgr,
            devices,
            ps_regs,
            use_u8_ids,
            gate,
            die,
            device_id,
            out,
            seen,
            set(),
        )
    return out


def _clock_gate_devices(value: Any, label: str) -> list[dict[str, Any]]:
    if value is None:
        return []
    out = []
    try:
        iterable = list(value)
    except TypeError as exc:
        raise AvdDtbError(f"{label} is not a list") from exc
    for index, item in enumerate(iterable):
        if not isinstance(item, dict):
            raise AvdDtbError(f"{label}[{index}] is not an object")
        paddr = item.get("paddr")
        if paddr is not None:
            paddr = int(paddr)
            if paddr < 0:
                raise AvdDtbError(f"{label}[{index}].paddr is negative")
        out.append(
            {
                "gate": int(item["gate"]),
                "die": int(item["die"]),
                "device_id": int(item["device_id"]),
                "name": str(item.get("name", "")),
                "parents": _u32_list(item.get("parents"), f"{label}[{index}].parents"),
                "virtual": bool(item.get("virtual", False)),
                "paddr": paddr,
            }
        )
    return out


def _infer_soc(machine: str | None, compatibles: list[str]) -> str:
    for compatible in compatibles:
        match = re.search(r"\b(t[0-9]{4})\b", compatible)
        if match:
            return match.group(1)
    if machine in {"j274", "j293", "j313", "j456", "j457"}:
        return "t8103"
    raise AvdDtbError("cannot infer Apple SoC; pass --soc or include it in info JSON")


def _reg0(node: Any, path: str) -> tuple[int, int]:
    try:
        base, size = node.get_reg(0)
    except Exception as exc:
        raise AvdDtbError(f"failed to read ADT reg[0] for {path}") from exc
    base = int(base)
    size = int(size)
    if size <= 0:
        raise AvdDtbError(f"ADT reg[0] for {path} has invalid size {size:#x}")
    return base, size


def _try_stream_id(dart_node: Any) -> int:
    # m1n1's AVD tracer accesses stream 0. Keep that as the fallback, but try
    # to preserve a simple child reg value if a future ADT exposes one.
    try:
        for child in dart_node:
            reg = child.getprop("reg", None) if hasattr(child, "getprop") else None
            if isinstance(reg, int):
                return reg
            if isinstance(reg, (list, tuple)) and reg:
                return int(reg[0])
    except Exception:
        pass
    return 0


def extract_avd_info_from_adt(
    adt: Any, machine: str | None = None, soc: str | None = None
) -> dict[str, Any]:
    """Extract Apple AVD DTB patch information from a live m1n1 ADT."""
    avd = _node_by_path(adt, AVD_ADT_PATH)
    dart = _node_by_path(adt, DART_AVD_ADT_PATH)
    avd_base, avd_size = _reg0(avd, AVD_ADT_PATH)
    dart_base, dart_size = _reg0(dart, DART_AVD_ADT_PATH)
    compatibles = _node_compatibles(avd) + _node_compatibles(dart)
    soc = soc or _infer_soc(machine, compatibles)
    avd_clock_gates = _node_u32_list(avd, "clock-gates", AVD_ADT_PATH)
    dart_clock_gates = _node_u32_list(dart, "clock-gates", DART_AVD_ADT_PATH)
    return {
        "machine": machine,
        "soc": soc,
        "sid": _try_stream_id(dart),
        "avd": {
            "adt_path": AVD_ADT_PATH,
            "base": avd_base,
            "size": avd_size,
            "compatible": _node_compatibles(avd),
            "clock_gates": avd_clock_gates,
            "clock_gate_devices": _pmgr_clock_gate_devices(adt, avd_clock_gates),
        },
        "dart": {
            "adt_path": DART_AVD_ADT_PATH,
            "base": dart_base,
            "size": dart_size,
            "compatible": _node_compatibles(dart),
            "clock_gates": dart_clock_gates,
            "clock_gate_devices": _pmgr_clock_gate_devices(adt, dart_clock_gates),
        },
    }


def load_info_json(path: pathlib.Path) -> dict[str, Any]:
    data = json.loads(path.read_text())
    return validate_info(data)


def validate_info(info: dict[str, Any]) -> dict[str, Any]:
    try:
        soc = str(info["soc"])
        avd = dict(info["avd"])
        dart = dict(info["dart"])
        sid = int(info.get("sid", 0))
        avd_base = int(avd["base"])
        avd_size = int(avd["size"])
        dart_base = int(dart["base"])
        dart_size = int(dart["size"])
    except Exception as exc:
        raise AvdDtbError("invalid AVD info JSON") from exc
    if avd_size <= 0 or dart_size <= 0:
        raise AvdDtbError("AVD info contains a zero-sized reg range")
    if not re.fullmatch(r"t[0-9]{4}", soc):
        raise AvdDtbError(f"unsupported Apple SoC name: {soc}")
    return {
        **info,
        "soc": soc,
        "sid": sid,
        "avd": {
            **avd,
            "adt_path": str(avd.get("adt_path", AVD_ADT_PATH)),
            "base": avd_base,
            "size": avd_size,
            "clock_gates": _u32_list(avd.get("clock_gates"), "avd.clock_gates"),
            "clock_gate_devices": _clock_gate_devices(
                avd.get("clock_gate_devices"), "avd.clock_gate_devices"
            ),
        },
        "dart": {
            **dart,
            "adt_path": str(dart.get("adt_path", DART_AVD_ADT_PATH)),
            "base": dart_base,
            "size": dart_size,
            "clock_gates": _u32_list(dart.get("clock_gates"), "dart.clock_gates"),
            "clock_gate_devices": _clock_gate_devices(
                dart.get("clock_gate_devices"), "dart.clock_gate_devices"
            ),
        },
    }


def write_info_json(path: pathlib.Path, info: dict[str, Any]) -> None:
    path.write_text(json.dumps(validate_info(info), indent=2, sort_keys=True) + "\n")


def _dtb_to_dts(dtb: pathlib.Path, dts: pathlib.Path) -> str:
    dtc = _tool("dtc")
    result = _run([dtc, "-I", "dtb", "-O", "dts", "-o", dts, dtb])
    return dts.read_text() + result.stderr


def _extract_braced_block(text: str, open_brace: int) -> str | None:
    if open_brace < 0 or open_brace >= len(text) or text[open_brace] != "{":
        return None
    depth = 0
    for index in range(open_brace, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[open_brace + 1 : index]
    return None


def _node_blocks(text: str, node_name_pattern: str) -> list[str]:
    blocks = []
    pattern = re.compile(rf"(?m)^\s*(?:[A-Za-z0-9_]+:\s*)?{node_name_pattern}\s*\{{")
    for match in pattern.finditer(text):
        open_brace = text.find("{", match.start(), match.end())
        block = _extract_braced_block(text, open_brace)
        if block is not None:
            blocks.append(block)
    return blocks


def _first_iommus_phandle(block: str) -> int | None:
    match = re.search(r"\biommus\s*=\s*<\s*([^>\s]+)", block)
    if match is None:
        return None
    try:
        return int(match.group(1), 0)
    except ValueError:
        return None


def _first_phandle(block: str) -> int | None:
    match = re.search(r"\b(?:linux,)?phandle\s*=\s*<\s*([^>\s]+)", block)
    if match is None:
        return None
    try:
        return int(match.group(1), 0)
    except ValueError:
        return None


def _dts_has_working_avd(text: str, require_pmgr_clock_gates: bool = False) -> bool:
    avd_iommus = []
    dart_phandles = set()
    for block in _node_blocks(text, r"avd@[0-9a-fA-F]+"):
        if re.search(r"apple,t[0-9]{4}-avd|apple,avd", block):
            if require_pmgr_clock_gates and "apple,pmgr-clock-gate-paddrs" not in block:
                continue
            phandle = _first_iommus_phandle(block)
            if phandle is not None:
                avd_iommus.append(phandle)
    for block in _node_blocks(text, r"iommu@[0-9a-fA-F]+"):
        if re.search(r"apple,t[0-9]{4}-dart|apple,dart", block):
            phandle = _first_phandle(block)
            if phandle is not None:
                dart_phandles.add(phandle)
    return any(phandle in dart_phandles for phandle in avd_iommus)


def dtb_has_avd(dtb: pathlib.Path, require_pmgr_clock_gates: bool = False) -> bool:
    with tempfile.TemporaryDirectory() as tmp:
        dts = pathlib.Path(tmp) / "base.dts"
        text = _dtb_to_dts(dtb, dts)
    return _dts_has_working_avd(text, require_pmgr_clock_gates)


def _next_phandle(dtb: pathlib.Path) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        dts = pathlib.Path(tmp) / "base.dts"
        text = _dtb_to_dts(dtb, dts)
    values = [0]
    for match in re.finditer(r"(?:linux,)?phandle\s*=\s*<([^>]+)>", text):
        for token in match.group(1).split():
            try:
                values.append(int(token, 0))
            except ValueError:
                pass
    return max(values) + 1


def _cells64(value: int) -> str:
    return f"0x{(value >> 32) & 0xffffffff:x} 0x{value & 0xffffffff:x}"


def _reg_cells(base: int, size: int) -> str:
    return f"<{_cells64(base)} {_cells64(size)}>"


def _u32_cells(values: list[int]) -> str:
    return " ".join(f"0x{value:x}" for value in values)


def _u64_cells(values: list[int]) -> str:
    return " ".join(_cells64(value) for value in values)


def _optional_u32_property(indent: str, name: str, values: list[int]) -> str:
    if not values:
        return ""
    return f"{indent}{name} = <{_u32_cells(values)}>;\n"


def _optional_u64_property(indent: str, name: str, values: list[int]) -> str:
    if not values:
        return ""
    return f"{indent}{name} = <{_u64_cells(values)}>;\n"


def _dts_string(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"')


def _optional_string_list_property(indent: str, name: str, values: list[str]) -> str:
    if not values:
        return ""
    strings = ", ".join(f'"{_dts_string(value)}"' for value in values)
    return f"{indent}{name} = {strings};\n"


def _clock_gate_paddrs(info_node: dict[str, Any]) -> list[int]:
    out = []
    seen = set()
    for device in info_node["clock_gate_devices"]:
        if device.get("paddr") is None or device.get("virtual", False):
            continue
        paddr = int(device["paddr"])
        if paddr not in seen:
            out.append(paddr)
            seen.add(paddr)
    return out


def _clock_gate_names(info_node: dict[str, Any]) -> list[str]:
    out = []
    seen = set()
    for device in info_node["clock_gate_devices"]:
        if not device.get("name") or device.get("virtual", False):
            continue
        name = str(device["name"])
        if name not in seen:
            out.append(name)
            seen.add(name)
    return out


def _overlay_dts(info: dict[str, Any], dart_phandle: int) -> str:
    info = validate_info(info)
    soc = info["soc"]
    avd = info["avd"]
    dart = info["dart"]
    sid = int(info.get("sid", 0))
    avd_clock_gates = _optional_u32_property(
        "                ", "apple,adt-clock-gates", avd["clock_gates"]
    )
    dart_clock_gates = _optional_u32_property(
        "                ", "apple,adt-clock-gates", dart["clock_gates"]
    )
    avd_dependency_paddrs = _clock_gate_paddrs(dart) + _clock_gate_paddrs(avd)
    avd_dependency_names = _clock_gate_names(dart) + _clock_gate_names(avd)
    avd_paddrs = _optional_u64_property(
        "                ", "apple,pmgr-clock-gate-paddrs", avd_dependency_paddrs
    )
    dart_paddrs = _optional_u64_property(
        "                ", "apple,pmgr-clock-gate-paddrs", _clock_gate_paddrs(dart)
    )
    avd_names = _optional_string_list_property(
        "                ", "apple,pmgr-clock-gate-names", avd_dependency_names
    )
    dart_names = _optional_string_list_property(
        "                ", "apple,pmgr-clock-gate-names", _clock_gate_names(dart)
    )
    return f"""/dts-v1/;
/plugin/;

/ {{
    fragment@0 {{
        target-path = "{GUEST_SOC_PATH}";
        __overlay__ {{
            #address-cells = <2>;
            #size-cells = <2>;

            dart_avd: iommu@{dart["base"]:x} {{
                compatible = "apple,{soc}-dart", "apple,dart";
                reg = {_reg_cells(dart["base"], dart["size"])};
{dart_clock_gates}                apple,adt-path = "{dart["adt_path"]}";
{dart_paddrs}{dart_names}                #iommu-cells = <1>;
                status = "okay";
                phandle = <0x{dart_phandle:x}>;
            }};

            avd@{avd["base"]:x} {{
                compatible = "apple,{soc}-avd", "apple,avd";
                reg = {_reg_cells(avd["base"], avd["size"])};
{avd_clock_gates}                apple,adt-path = "{avd["adt_path"]}";
{avd_paddrs}{avd_names}                iommus = <&dart_avd 0x{sid:x}>;
                status = "okay";
            }};
        }};
    }};
}};
"""


def patch_dtb_file(input_dtb: pathlib.Path, output_dtb: pathlib.Path, info: dict[str, Any]) -> bool:
    """Patch a DTB file. Return True when a patch was applied."""
    info = validate_info(info)
    require_pmgr_clock_gates = bool(
        _clock_gate_paddrs(info["avd"]) or _clock_gate_paddrs(info["dart"])
    )
    if dtb_has_avd(input_dtb, require_pmgr_clock_gates):
        if input_dtb.resolve() != output_dtb.resolve():
            output_dtb.write_bytes(input_dtb.read_bytes())
        return False

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        overlay_dts = tmp_path / "apple-avd-overlay.dts"
        overlay_dtbo = tmp_path / "apple-avd-overlay.dtbo"
        overlay_dts.write_text(_overlay_dts(info, _next_phandle(input_dtb)))
        _run([
            _tool("dtc"),
            "-@",
            "-I",
            "dts",
            "-O",
            "dtb",
            "-o",
            overlay_dtbo,
            overlay_dts,
        ])
        _run([_tool("fdtoverlay"), "-i", input_dtb, "-o", output_dtb, overlay_dtbo])
    return True


def patch_dtb_bytes(dtb: bytes, info: dict[str, Any]) -> tuple[bytes, bool]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        input_dtb = tmp_path / "input.dtb"
        output_dtb = tmp_path / "output.dtb"
        input_dtb.write_bytes(dtb)
        changed = patch_dtb_file(input_dtb, output_dtb, info)
        return output_dtb.read_bytes(), changed


def _payload_dtb_offset(payload: bytes, m1n1_bin: pathlib.Path | None) -> int:
    if m1n1_bin is not None and m1n1_bin.exists():
        offset = len(m1n1_bin.read_bytes())
        if payload[offset : offset + 4] == FDT_MAGIC:
            return offset
    offset = payload.find(FDT_MAGIC)
    if offset < 0:
        raise AvdDtbError("payload does not contain an FDT blob")
    return offset


def patch_payload_bytes(
    payload: bytes, info: dict[str, Any], m1n1_bin: pathlib.Path | None = None
) -> tuple[bytes, bool]:
    offset = _payload_dtb_offset(payload, m1n1_bin)
    if len(payload) < offset + 8:
        raise AvdDtbError("payload FDT header is truncated")
    dtb_size = int.from_bytes(payload[offset + 4 : offset + 8], "big")
    dtb_end = offset + dtb_size
    if dtb_size <= 0 or dtb_end > len(payload):
        raise AvdDtbError("payload FDT totalsize is invalid")
    patched_dtb, changed = patch_dtb_bytes(payload[offset:dtb_end], info)
    return payload[:offset] + patched_dtb + payload[dtb_end:], changed


def patch_payload_file(
    input_payload: pathlib.Path,
    output_payload: pathlib.Path,
    info: dict[str, Any],
    m1n1_bin: pathlib.Path | None = None,
) -> bool:
    patched, changed = patch_payload_bytes(input_payload.read_bytes(), info, m1n1_bin)
    output_payload.write_bytes(patched)
    return changed


def _describe_clock_gate_devices(info_node: dict[str, Any]) -> str:
    parts = []
    for device in info_node["clock_gate_devices"]:
        name = device["name"] or f"id{device['device_id']:#x}"
        if device.get("virtual", False):
            suffix = "virtual"
        elif device.get("paddr") is None:
            suffix = "unmapped"
        else:
            suffix = f"{int(device['paddr']):#x}"
        parts.append(f"{name}@{suffix}")
    return " ".join(parts)


def clock_gate_warnings(info: dict[str, Any]) -> list[str]:
    info = validate_info(info)
    warnings = []
    for label in ("dart", "avd"):
        node = info[label]
        if node["clock_gates"] and not _clock_gate_paddrs(node):
            warnings.append(
                f"{label} has ADT clock-gates but no PMGR paddr mapping"
            )
    return warnings


def describe_info(info: dict[str, Any]) -> str:
    info = validate_info(info)
    clock_gates = (
        f" clock-gates: avd=[{_u32_cells(info['avd']['clock_gates'])}] "
        f"dart=[{_u32_cells(info['dart']['clock_gates'])}]"
    )
    pmgr_devices = (
        f" pmgr: avd=[{_describe_clock_gate_devices(info['avd'])}] "
        f"dart=[{_describe_clock_gate_devices(info['dart'])}]"
    )
    return (
        f"soc={info['soc']} avd={info['avd']['base']:#x}+{info['avd']['size']:#x} "
        f"dart={info['dart']['base']:#x}+{info['dart']['size']:#x} sid={info['sid']:#x}"
        f"{clock_gates}{pmgr_devices}"
    )


def _main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    patch_dtb = sub.add_parser("patch-dtb", help="patch a standalone DTB")
    patch_dtb.add_argument("--info-json", type=pathlib.Path, required=True)
    patch_dtb.add_argument("--input", type=pathlib.Path, required=True)
    patch_dtb.add_argument("--output", type=pathlib.Path, required=True)

    patch_payload = sub.add_parser("patch-payload", help="patch a raw m1n1+DTB payload")
    patch_payload.add_argument("--info-json", type=pathlib.Path, required=True)
    patch_payload.add_argument("--input", type=pathlib.Path, required=True)
    patch_payload.add_argument("--output", type=pathlib.Path, required=True)
    patch_payload.add_argument("--m1n1-bin", type=pathlib.Path)

    args = parser.parse_args()
    try:
        info = load_info_json(args.info_json)
        if args.command == "patch-dtb":
            changed = patch_dtb_file(args.input, args.output, info)
        elif args.command == "patch-payload":
            changed = patch_payload_file(args.input, args.output, info, args.m1n1_bin)
        else:
            raise AssertionError(args.command)
        action = "patched" if changed else "already-present"
        print(f"{action}: {describe_info(info)}", file=sys.stderr)
        return 0
    except (AvdDtbError, subprocess.CalledProcessError) as exc:
        print(f"apple-avd-dtb: {exc}", file=sys.stderr)
        if isinstance(exc, subprocess.CalledProcessError) and exc.stderr:
            print(exc.stderr, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(_main())
