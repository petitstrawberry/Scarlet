"""Pinned Android 13 framework/JNI fixture inputs, not a full Android boot."""
import json
from pathlib import Path
import re
import shutil

from art_inputs import SDK_SHA256, sha256, validate_dex, verify_art_runtime
from stock_art import verify_stock_inputs

SOURCE = Path(__file__).resolve().parent
ART_FRAGMENT = "apex/com.android.art/etc/classpaths/bootclasspath.pb"
SYSTEM_FRAGMENT = "system/etc/classpaths/bootclasspath.pb"
PROGRAM = "system/bin/app_process64"


def protobuf_fields(data):
    """Bounded subset used by the pinned classpaths.proto, rejecting ambiguity."""
    offset = 0

    def number():
        nonlocal offset
        result = 0
        for shift in range(0, 70, 7):
            if offset == len(data):
                raise ValueError("truncated classpath varint")
            byte = data[offset]
            offset += 1
            if shift == 63 and byte > 1:
                raise ValueError("oversized classpath varint")
            result |= (byte & 127) << shift
            if not byte & 128:
                return result
        raise ValueError("oversized classpath varint")

    while offset < len(data):
        tag = number()
        if tag >> 3 == 0:
            raise ValueError("invalid classpath field")
        if tag & 7 == 0:
            value = number()
        elif tag & 7 == 2:
            size = number()
            if size > len(data) - offset:
                raise ValueError("truncated classpath field")
            value = data[offset:offset + size]
            offset += size
        else:
            raise ValueError("unsupported classpath wire type")
        yield tag >> 3, value


def boot_classpath(root):
    """Android 13 derive_classpath order: ART, system, sorted remaining APEXes.

    This fixture accepts the pinned release's numeric API bounds only. Unknown
    schema or prerelease codenames require explicit support, not guessed order.
    """
    fragments = [ART_FRAGMENT, SYSTEM_FRAGMENT]
    fragments += [str(p.relative_to(root)) for p in sorted(root.glob("apex/*/etc/classpaths/bootclasspath.pb"))
                  if str(p.relative_to(root)) != ART_FRAGMENT]
    paths = []
    for fragment in fragments:
        data = (root / fragment).read_bytes()
        if len(data) > 64 * 1024:
            raise ValueError("classpath fragment exceeds fixture limit")
        for tag, record in protobuf_fields(data):
            if tag != 1 or not isinstance(record, bytes):
                raise ValueError("unexpected classpath record")
            fields = {}
            for key, value in protobuf_fields(record):
                if key not in (1, 2, 3, 4) or key in fields:
                    raise ValueError("unknown or duplicate classpath field")
                fields[key] = value
            if not isinstance(fields.get(1), bytes) or fields.get(2) not in (1, 3):
                raise ValueError("invalid boot classpath entry")
            path = fields[1].decode("utf-8")
            if (not re.fullmatch(r"/(system/framework|apex/[A-Za-z0-9_.]+/javalib)/[A-Za-z0-9_.-]+\.jar", path)
                    or ".." in Path(path).parts):
                raise ValueError("invalid boot jar path")
            if fragment.startswith("apex/") and not path.startswith("/" + "/".join(fragment.split("/")[:2]) + "/"):
                raise ValueError("classpath jar escapes its APEX")
            minimum, maximum = fields.get(3, b""), fields.get(4, b"")
            if any(not isinstance(v, bytes) or (v and not re.fullmatch(rb"[0-9]{1,3}", v)) for v in (minimum, maximum)):
                raise ValueError("unsupported classpath API bound")
            if fields[2] == 1 and int(minimum or b"0") <= 33 <= int(maximum or b"999"):
                if path in paths:
                    raise ValueError("duplicate boot jar")
                paths.append(path)
    if not paths:
        raise ValueError("empty boot classpath")
    return paths, fragments


def verify_framework_inputs(root, runtime, stock):
    verify_art_runtime(runtime)
    verify_stock_inputs(stock, runtime)
    metadata = json.loads((root / "framework-inputs.json").read_text())
    if (metadata.get("result") != "PREPARED" or metadata.get("kind") != "stock-framework-inputs-v1"
            or metadata.get("sdk", {}).get("sha256") != SDK_SHA256
            or metadata.get("base_manifest_sha256") != sha256(runtime / "art-runtime.json")
            or metadata.get("stock_manifest_sha256") != sha256(stock / "stock-inputs.json")):
        raise ValueError("incomplete or mismatched framework inputs")
    files = metadata["files"]
    if PROGRAM not in files:
        raise ValueError("missing stock app_process64")
    for name, digest in files.items():
        if not name or Path(name).is_absolute() or ".." in Path(name).parts or any(c in name for c in "\0\r\n\x1f"):
            raise ValueError("invalid framework input path")
        path = root / name
        if not path.resolve().is_relative_to(root.resolve()) or path.is_symlink() or sha256(path) != digest:
            raise ValueError("changed framework input: " + name)
    actual = {str(p.relative_to(root)) for p in root.rglob("*") if p.is_file() or p.is_symlink()}
    if actual != set(files) | {"framework-inputs.json"} or any(p.is_symlink() for p in root.rglob("*")):
        raise ValueError("unrecorded framework input")
    classpath, fragments = boot_classpath(root)
    if (classpath != metadata["boot_class_path"] or fragments != metadata["fragments"]
            or any(p.lstrip("/") not in files for p in classpath)):
        raise ValueError("framework classpath differs from image fragments")
    return metadata


def verify_framework_probe(build, services=False):
    record = json.loads((build / "build.json").read_text())
    if (record.get("main_class") != "ScarletFrameworkHello" or not record.get("framework_sdk_sha256")
            or record.get("source_sha256") != sha256(SOURCE / "framework/ScarletFrameworkHello.java")
            or record.get("dex_sha256") != validate_dex(build / "classes.dex")):
        raise ValueError("changed or mismatched framework probe")
    if services and record.get("framework_service_sha256") != sha256(SOURCE / "framework/ScarletFrameworkService.java"):
        raise ValueError("changed or missing Java service probe")
    return record


def framework_launch(metadata, services=False, apk=False):
    return {
        "argv": ["/" + PROGRAM, "-Xint", "-Xusejit:false", "-Ximage:/system/nonexistent/boot.art",
                 "-Xnoimage-dex2oat", "-Xms16m", "-Xmx256m", "/system/bin", "--application", "ScarletApkProbe" if apk else "ScarletFrameworkService" if services else "ScarletFrameworkHello"],
        "env": ["PATH=/system/bin:/apex/com.android.art/bin", "ANDROID_ROOT=/system", "ANDROID_DATA=/data",
                "ANDROID_ART_ROOT=/apex/com.android.art", "ANDROID_I18N_ROOT=/apex/com.android.i18n",
                "ANDROID_TZDATA_ROOT=/apex/com.android.tzdata", "ANDROID_LOG_TAGS=*:v",
                "BOOTCLASSPATH=" + ":".join(metadata["boot_class_path"]), "CLASSPATH=/data/local/tmp/classes.dex"],
    }


def stage_framework_inputs(root, runtime, stock, tree):
    metadata = verify_framework_inputs(root, runtime, stock)
    for name, digest in metadata["files"].items():
        destination = tree / name
        if destination.is_symlink() or (destination.exists() and sha256(destination) != digest):
            raise ValueError("framework would replace a different staged input: " + name)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / name, destination)
        if sha256(destination) != digest:
            raise ValueError("framework input changed during staging: " + name)
    # AndroidRuntime appends its property-driven JIT option after argv options.
    # Explicit interpreter-only fixture policy; never alter stock image inputs.
    properties = tree / "data/local/tmp/property-values.prop"
    original = sha256(properties)
    values = dict(line.split("=", 1) for line in properties.read_text().splitlines())
    if not values.get("ro.product.cpu.abilist64"):
        raise ValueError("prepare stock inputs with CPU ABI bootstrap first")
    previous = values.get("dalvik.vm.usejit")
    values["dalvik.vm.usejit"] = "false"
    properties.write_text("".join(f"{key}={value}\n" for key, value in sorted(values.items())))
    return {"original_properties_sha256": original, "staged_properties_sha256": sha256(properties),
            "fixture_overrides": {"dalvik.vm.usejit": {"original": previous, "value": "false"}}}
