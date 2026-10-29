#!/usr/bin/env python3
"""Distinguish Android boot, APK execution, input, and PNG capture evidence."""
import argparse
import base64
import binascii
import json
from pathlib import Path
import re
import struct
import zlib

MAX_PNG = 16 * 1024 * 1024
MAX_PIXELS = 16 * 1024 * 1024


def analyze_services(data):
    lines = data.replace(b"\r", b"").decode("utf-8", errors="replace").splitlines()
    records = [line for line in lines if line.startswith("SCARLET_SERVICES_")]
    expected = ["CONFIGURED", "REGISTERED", "LOOKUP_CALL_LIST_OK", "DENIED_ADD_FIND_LIST",
                "OWNER_EXITING", "OWNER_DEATH_OK", "EXIT owner 0", "EXIT client 0", "EXIT denied 0", "OK"]
    required = ["SCARLET_SERVICES_" + item for item in expected]
    valid = all(records.count(item) == 1 for item in required)
    pids = []
    for mode in ("manager", "owner", "client", "denied"):
        matches = [re.fullmatch("SCARLET_SERVICES_SPAWN " + mode + r" ([1-9][0-9]*)", r) for r in records]
        pids += [m.group(1) for m in matches if m]
        valid &= sum(bool(m) for m in matches) == 1
    valid &= len(set(pids)) == 4
    for mode in ("owner", "client", "denied"):
        valid &= records.count("SCARLET_SERVICES_PEER " + mode) == 1
    stopped = [r for r in records if re.fullmatch(r"SCARLET_SERVICES_EXIT manager (-9|137)", r)]
    valid &= len(stopped) == 1 and len(records) == len(required) + 8
    valid &= not any("_FAIL" in line for line in lines)
    stock = [line for line in lines if line.startswith(("SCARLET_ANDROID_STOCK_", "SCARLET_PROPERTY_"))]
    stock = ["SCARLET_PROPERTY_SNAPSHOT_OK" if re.fullmatch(r"SCARLET_PROPERTY_SNAPSHOT_OK [1-9][0-9]*", line) else line for line in stock]
    valid &= stock == ["SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK",
                       "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
                       "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK"]
    if valid:
        position = lambda event: lines.index("SCARLET_SERVICES_" + event)
        valid &= position("CONFIGURED") < lines.index("SCARLET_ANDROID_STOCK_OK") < position("REGISTERED") < position("LOOKUP_CALL_LIST_OK") < position("OWNER_EXITING") < position("OWNER_DEATH_OK") < position("EXIT client 0") < position("OK")
        valid &= position("REGISTERED") < position("DENIED_ADD_FIND_LIST") < position("OWNER_EXITING")
        valid &= all(position("EXIT " + mode + " 0") < lines.index(stopped[0]) < position("OK") for mode in ("owner", "client", "denied"))
    return {"result": "PASS" if valid else "INCOMPLETE", "records": records}


def analyze_selinux(data):
    expected = ["SCARLET_SHIELD_CONFIGURED",
                "SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK",
                "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
                "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK",
                "SCARLET_ANDROID_PROGRAM_START", "SCARLET_SHIELD_BEGIN",
                "SCARLET_SHIELD_RUNTIME /system/lib64/libselinux.so", "SCARLET_SHIELD_STATUS_OK",
                "SCARLET_SHIELD_ALLOW_DENY_OK", "SCARLET_SHIELD_UNKNOWN_DENIED",
                "SCARLET_SHIELD_MUTATION_DENIED", "SCARLET_SHIELD_EXEC_LABEL_OK",
                "SCARLET_SHIELD_FORK_EXEC_OK", "SCARLET_SHIELD_OK",
                "SCARLET_ANDROID_PROGRAM_EXIT 0", "SCARLET_ANDROID_PROGRAM_DONE"]
    lines = data.replace(b"\r", b"").decode("utf-8", errors="replace").splitlines()
    records = [line for line in lines if line.startswith(
        ("SCARLET_SHIELD_", "SCARLET_ANDROID_PROGRAM_", "SCARLET_ANDROID_STOCK_", "SCARLET_PROPERTY_"))]
    records = ["SCARLET_PROPERTY_SNAPSHOT_OK" if
               re.fullmatch(r"SCARLET_PROPERTY_SNAPSHOT_OK [1-9][0-9]*", line) else line for line in records]
    return {"result": "PASS" if records == expected else "INCOMPLETE", "records": records}


def analyze_libbinder(data):
    """Require stock setup, actual RPC/lifetime checks, and both process exits."""
    expected = ["SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK",
                "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
                "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK",
                "SCARLET_ANDROID_PROGRAM_START", "SCARLET_LIBBINDER_BEGIN",
                "SCARLET_LIBBINDER_RUNTIME /system/lib64/libbinder.so", "SCARLET_LIBBINDER_SERVER_READY",
                "SCARLET_LIBBINDER_CONTEXT_OK"]
    for cycle in (1, 2):
        expected += [f"SCARLET_LIBBINDER_CYCLE {cycle} {step}" for step in
                     ("HANDLE_OK", "ROUNDTRIP_OK", "CALL_OK", "STATUS_OK",
                      "STRONG_ZERO_WEAK_LIVE", "REFS_RELEASED")]
    expected += ["SCARLET_LIBBINDER_DEATH_UNLINK_OK", "SCARLET_LIBBINDER_DEATH_CALLBACK_OK",
                 "SCARLET_LIBBINDER_DEATH_CLEAR_OK", "SCARLET_LIBBINDER_DEAD_OBJECT_OK",
                 "SCARLET_LIBBINDER_DEATH_QUIET_OK",
                 "SCARLET_LIBBINDER_SERVER_EXIT 0", "SCARLET_LIBBINDER_OK",
                 "SCARLET_ANDROID_PROGRAM_EXIT 0", "SCARLET_ANDROID_PROGRAM_DONE"]
    lines = data.replace(b"\r", b"").decode("utf-8", errors="replace").splitlines()
    records = [line for line in lines if line.startswith(
        ("SCARLET_LIBBINDER_", "SCARLET_ANDROID_PROGRAM_", "SCARLET_ANDROID_STOCK_", "SCARLET_PROPERTY_"))]
    records = ["SCARLET_PROPERTY_SNAPSHOT_OK" if
               re.fullmatch(r"SCARLET_PROPERTY_SNAPSHOT_OK [1-9][0-9]*", line) else line for line in records]
    return {"result": "PASS" if records == expected else "INCOMPLETE", "records": records}


def analyze_art(data, stock=False):
    """A Java marker alone, or a launcher exit alone, is not an ART pass."""
    expected = ["SCARLET_ANDROID_ART_START", "SCARLET_ART_BEGIN", "Scarlet ART: 42",
                "SCARLET_ART_OK", "SCARLET_ANDROID_ART_EXIT 0", "SCARLET_ANDROID_ART_OK"]
    lines = data.replace(b"\r", b"").decode("utf-8", errors="replace").splitlines()
    if stock:
        expected = ["SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK",
                    "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
                    "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK"] + expected
    records = [line for line in lines if line.startswith(
        ("SCARLET_ANDROID_ART_", "SCARLET_ART_", "Scarlet ART:") +
        (("SCARLET_ANDROID_STOCK_", "SCARLET_PROPERTY_") if stock else ()))]
    records = ["SCARLET_PROPERTY_SNAPSHOT_OK" if stock and
               re.fullmatch(r"SCARLET_PROPERTY_SNAPSHOT_OK [1-9][0-9]*", line) else line for line in records]
    return {"result": "PASS" if records == expected else "INCOMPLETE", "records": records}


def analyze_direct(data, dynamic=False, binder=False):
    """Require child evidence as well as native wait status and suite marker."""
    object_steps = ["BEGIN", "REJECT_OK", "TRANSFER_OK", "REFS strong-zero weak-live",
                    "REFS balanced-before-close", "OK"]
    expected = [("/bin/bionic-static", name) for name in ("libc", "thread", "shared", "signal", "exec", "xattr", "mremap")]
    if binder:
        expected.append(("/bin/bionic-static", "binder"))
    if dynamic:
        expected += [("/bin/bionic-dynamic", name) for name in ("libc", "thread", "shared", "signal", "dynamic", "exec", "xattr", "mremap")]
        if binder:
            expected.append(("/bin/bionic-dynamic", "binder"))
    expected.append(("/bin/bionic-static", "binder-absent"))
    cases, current, complete = [], None, False
    for line in data.replace(b"\r", b"").decode("utf-8", errors="replace").splitlines():
        match = re.fullmatch(r"SCARLET_ANDROID_DIRECT_CASE (\S+) (\S+)", line)
        if match:
            current = {"program": match[1], "case": match[2], "began": False, "passed": False, "status": None}
            if current["case"] == "binder":
                current["object_steps"] = []
            cases.append(current)
        elif current is not None:
            if line == "SCARLET_BIONIC_BEGIN " + current["case"]:
                current["began"] = True
            elif line == "SCARLET_BIONIC_OK " + current["case"]:
                current["passed"] = current["began"]
            elif current["case"] == "binder" and line.startswith("SCARLET_BINDER_OBJECTS_"):
                step = line.removeprefix("SCARLET_BINDER_OBJECTS_")
                current["object_steps"].append(step if step.startswith("REFS ") else step.split()[0])
            else:
                match = re.fullmatch(r"SCARLET_ANDROID_DIRECT_EXIT (\S+) (\S+) (-?\d+)", line)
                if match and (match[1], match[2]) == (current["program"], current["case"]):
                    current["status"] = int(match[3])
                    current = None
        if line == "SCARLET_ANDROID_DIRECT_OK":
            complete = True
    passed = (complete and [(c["program"], c["case"]) for c in cases] == expected and
              all(c["passed"] and c["status"] == 0 and
                  (c["case"] != "binder" or c["object_steps"] == object_steps) for c in cases))
    return {"result": "PASS" if passed else "INCOMPLETE", "cases": cases}


def validate_png(data):
    """Validate bounded noninterlaced RGB/RGBA screencap output, including CRCs."""
    if len(data) > MAX_PNG or not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("invalid PNG signature or size")
    offset, dimensions, compressed, ended = 8, None, bytearray(), False
    while offset < len(data):
        if offset + 12 > len(data):
            raise ValueError("truncated PNG chunk")
        size, kind = struct.unpack_from(">I4s", data, offset)
        end = offset + 12 + size
        if end > len(data):
            raise ValueError("truncated PNG payload")
        content = data[offset + 8:end - 4]
        if zlib.crc32(kind + content) != struct.unpack_from(">I", data, end - 4)[0]:
            raise ValueError("PNG CRC mismatch")
        if dimensions is None and kind != b"IHDR":
            raise ValueError("PNG must start with IHDR")
        if kind == b"IHDR":
            if dimensions or size != 13:
                raise ValueError("invalid IHDR")
            w, h, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", content)
            if not w or not h or w * h > MAX_PIXELS or depth != 8 or color not in (2, 6) or any((compression, filtering, interlace)):
                raise ValueError("unsupported screencap dimensions/format")
            dimensions = (w, h, 3 if color == 2 else 4)
        elif kind == b"IDAT":
            compressed.extend(content)
        elif kind == b"IEND":
            if size or end != len(data):
                raise ValueError("invalid IEND/trailing data")
            ended = True
            break
        elif not kind[0] & 0x20 and kind != b"PLTE":
            raise ValueError("unknown critical PNG chunk")
        offset = end
    if not ended or not compressed:
        raise ValueError("incomplete PNG")
    w, h, channels = dimensions
    stride = 1 + w * channels
    expected = stride * h
    decoder = zlib.decompressobj()
    pixels = decoder.decompress(compressed, expected + 1)
    if len(pixels) != expected or not decoder.eof or decoder.unused_data or decoder.unconsumed_tail:
        raise ValueError("invalid PNG decompressed size/stream")
    if any(pixels[row * stride] > 4 for row in range(h)):
        raise ValueError("invalid PNG row filter")
    return {"width": w, "height": h, "channels": channels}


def analyze(data, apk=False):
    lines = data.replace(b"\r", b"").splitlines()
    marker = b"SCARLET_APK_FRAME" if apk else b"SCARLET_FRAME"
    frame, error = None, None
    try:
        begin = lines.index(marker + b"_BEGIN")
        end = lines.index(marker + b"_END", begin + 1)
        encoded = b"".join(lines[begin + 1:end])
        if len(encoded) > (MAX_PNG + 2) // 3 * 4:
            raise ValueError("encoded capture exceeds limit")
        frame = base64.b64decode(encoded, validate=True)
        dimensions = validate_png(frame)
    except (ValueError, binascii.Error, zlib.error) as exc:
        dimensions, frame, error = None, None, str(exc)
    result = {
        "boot_completed": b"SCARLET_ANDROID_BOOT_OK" in lines,
        "apk_resumed": b"SCARLET_ANDROID_APK_RESUMED" in lines,
        "apk_input": b"SCARLET_ANDROID_APK_INPUT_OK" in lines,
        "apk_completed": b"SCARLET_ANDROID_APK_OK" in lines,
        "apk_failure": any(line.startswith(b"SCARLET_ANDROID_APK_FAIL:") for line in lines),
        "capture": dimensions,
        "capture_error": error,
        "sws_presentation": "not tested",
    }
    result["result"] = "PASS" if (result["boot_completed"] and frame is not None and
        (not apk or (result["apk_resumed"] and result["apk_input"] and result["apk_completed"] and not result["apk_failure"]))) else "INCOMPLETE"
    return result, frame


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apk", action="store_true")
    mode.add_argument("--art", action="store_true", help="require Java output and normal ART exit")
    parser.add_argument("--stock", action="store_true", help="also require stock property/linkerconfig bootstrap")
    args = parser.parse_args()
    if args.stock and not args.art:
        parser.error("--stock requires --art")
    data = args.serial.read_bytes()
    result, frame = (analyze_art(data, stock=args.stock), None) if args.art else analyze(data, apk=args.apk)
    args.output.mkdir(parents=True, exist_ok=False)
    if frame is not None:
        (args.output / "frame.png").write_bytes(frame)
    (args.output / "evidence.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 0 if result["result"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
