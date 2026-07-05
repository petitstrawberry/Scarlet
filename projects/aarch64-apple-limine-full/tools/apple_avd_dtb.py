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
GUEST_SOC_PATH = "/soc"


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
    return {
        "machine": machine,
        "soc": soc,
        "sid": _try_stream_id(dart),
        "avd": {
            "adt_path": AVD_ADT_PATH,
            "base": avd_base,
            "size": avd_size,
            "compatible": _node_compatibles(avd),
        },
        "dart": {
            "adt_path": DART_AVD_ADT_PATH,
            "base": dart_base,
            "size": dart_size,
            "compatible": _node_compatibles(dart),
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
        "avd": {**avd, "base": avd_base, "size": avd_size},
        "dart": {**dart, "base": dart_base, "size": dart_size},
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


def _dts_has_working_avd(text: str) -> bool:
    avd_iommus = []
    dart_phandles = set()
    for block in _node_blocks(text, r"avd@[0-9a-fA-F]+"):
        if re.search(r"apple,t[0-9]{4}-avd|apple,avd", block):
            phandle = _first_iommus_phandle(block)
            if phandle is not None:
                avd_iommus.append(phandle)
    for block in _node_blocks(text, r"iommu@[0-9a-fA-F]+"):
        if re.search(r"apple,t[0-9]{4}-dart|apple,dart", block):
            phandle = _first_phandle(block)
            if phandle is not None:
                dart_phandles.add(phandle)
    return any(phandle in dart_phandles for phandle in avd_iommus)


def dtb_has_avd(dtb: pathlib.Path) -> bool:
    with tempfile.TemporaryDirectory() as tmp:
        dts = pathlib.Path(tmp) / "base.dts"
        text = _dtb_to_dts(dtb, dts)
    return _dts_has_working_avd(text)


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


def _overlay_dts(info: dict[str, Any], dart_phandle: int) -> str:
    info = validate_info(info)
    soc = info["soc"]
    avd = info["avd"]
    dart = info["dart"]
    sid = int(info.get("sid", 0))
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
                #iommu-cells = <1>;
                status = "okay";
                phandle = <0x{dart_phandle:x}>;
            }};

            avd@{avd["base"]:x} {{
                compatible = "apple,{soc}-avd", "apple,avd";
                reg = {_reg_cells(avd["base"], avd["size"])};
                iommus = <&dart_avd 0x{sid:x}>;
                status = "okay";
            }};
        }};
    }};
}};
"""


def patch_dtb_file(input_dtb: pathlib.Path, output_dtb: pathlib.Path, info: dict[str, Any]) -> bool:
    """Patch a DTB file. Return True when a patch was applied."""
    if dtb_has_avd(input_dtb):
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


def describe_info(info: dict[str, Any]) -> str:
    info = validate_info(info)
    return (
        f"soc={info['soc']} avd={info['avd']['base']:#x}+{info['avd']['size']:#x} "
        f"dart={info['dart']['base']:#x}+{info['dart']['size']:#x} sid={info['sid']:#x}"
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
