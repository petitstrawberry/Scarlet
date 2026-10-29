"""Host identity/orchestration tests; simulated inputs are not runtime evidence."""
import hashlib
import json
from pathlib import Path
import shutil
import struct
import tempfile
import unittest
from unittest.mock import patch
import zlib

from art_inputs import SDK_SHA256, sha256
from evidence import analyze_framework
from framework_inputs import (SOURCE, PROGRAM, ART_FRAGMENT, SYSTEM_FRAGMENT, boot_classpath,
                              verify_framework_inputs, verify_framework_probe, stage_framework_inputs)
from stock_art import stage_stock_inputs
from test_art_runner import runner, runtime_fixture, stock_fixture


def varint(value):
    data = bytearray()
    while value > 127:
        data.append((value & 127) | 128)
        value >>= 7
    return bytes(data) + bytes([value])


def string_field(tag, value):
    value = value.encode() if isinstance(value, str) else value
    return varint(tag * 8 + 2) + varint(len(value)) + value


def jar(path, kind=1, minimum="", maximum=""):
    data = string_field(1, path) + bytes([16, kind])
    if minimum:
        data += string_field(3, minimum)
    if maximum:
        data += string_field(4, maximum)
    return string_field(1, data)


def put(root, name, data):
    target = root / name
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)
    return target


def fixture(base):
    runtime, stock, root, build = [base / name for name in ("runtime", "stock", "framework", "probe")]
    runtime.mkdir()
    runtime_fixture(runtime)
    info = stock_fixture(stock, runtime)
    properties = stock / "data/local/tmp/property-values.prop"
    properties.write_text("ro.product.cpu.abilist64=arm64-v8a\ndalvik.vm.usejit=true\n")
    info["files"]["data/local/tmp/property-values.prop"] = sha256(properties)
    (stock / "stock-inputs.json").write_text(json.dumps(info))
    put(root, ART_FRAGMENT, jar("/apex/com.android.art/javalib/core-oj.jar"))
    put(root, SYSTEM_FRAGMENT, jar("/system/framework/framework.jar"))
    for name in ("apex/com.android.art/javalib/core-oj.jar", "system/framework/framework.jar", PROGRAM):
        put(root, name, (runtime / name).read_bytes() if (runtime / name).exists() else name.encode())
    classpath, fragments = boot_classpath(root)
    metadata = {"result": "PREPARED", "kind": "stock-framework-inputs-v1", "sdk": {"sha256": SDK_SHA256},
                "base_manifest_sha256": sha256(runtime / "art-runtime.json"),
                "stock_manifest_sha256": sha256(stock / "stock-inputs.json"),
                "boot_class_path": classpath, "fragments": fragments,
                "files": {str(p.relative_to(root)): sha256(p) for p in root.rglob("*") if p.is_file()}}
    (root / "framework-inputs.json").write_text(json.dumps(metadata))
    dex = bytearray(112)
    dex[:8] = b"dex\n035\0"
    struct.pack_into("<III", dex, 32, 112, 112, 0x12345678)
    dex[12:32] = hashlib.sha1(dex[32:]).digest()
    struct.pack_into("<I", dex, 8, zlib.adler32(dex[12:]))
    put(build, "classes.dex", dex)
    (build / "build.json").write_text(json.dumps({"main_class": "ScarletFrameworkHello", "framework_sdk_sha256": "simulated SDK",
        "source_sha256": sha256(SOURCE / "framework/ScarletFrameworkHello.java"), "dex_sha256": sha256(build / "classes.dex")}))
    return runtime, stock, root, build


def successful_log():
    return "\n" + "\n".join([
        "SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK 219",
        "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
        "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK",
        "SCARLET_ANDROID_PROGRAM_START", "SCARLET_FRAMEWORK_BEGIN", "SCARLET_FRAMEWORK_PARCEL_BINDER_OK",
        "SCARLET_FRAMEWORK_LOOPER_OK", "SCARLET_FRAMEWORK_OK", "SCARLET_ANDROID_PROGRAM_EXIT 0",
        "SCARLET_ANDROID_PROGRAM_DONE"]) + "\n"


class FrameworkTest(unittest.TestCase):
    def test_classpath_preserves_image_order_and_release_api_bounds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            art = "/apex/com.android.art/javalib/a.jar"
            platform = "/system/framework/f.jar"
            last = "/apex/com.android.zz/javalib/z.jar"
            first = "/apex/com.android.aa/javalib/a.jar"
            put(root, ART_FRAGMENT, jar(art) + jar(art, 3))
            put(root, SYSTEM_FRAGMENT, jar(platform) + jar("/system/framework/future.jar", minimum="34") +
                jar("/system/framework/past.jar", maximum="32"))
            put(root, "apex/com.android.zz/etc/classpaths/bootclasspath.pb", jar(last, minimum="33"))
            put(root, "apex/com.android.aa/etc/classpaths/bootclasspath.pb", jar(first, maximum="33"))
            self.assertEqual(boot_classpath(root)[0], [art, platform, first, last])
            for invalid in [b"\x0a\xff", b"\x00\x00", jar(art) + jar(art), jar("/system/framework/escape.jar"),
                            jar(art, minimum="Tiramisu"), jar(art, 2), string_field(1, string_field(1, art) * 2 + b"\x10\x01")]:
                with self.subTest(invalid=invalid):
                    put(root, ART_FRAGMENT, invalid)
                    with self.assertRaises(ValueError):
                        boot_classpath(root)

    def test_framework_inputs_and_probe_are_bound_and_staging_preserves_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            runtime, stock, root, build = fixture(base)
            verify_framework_inputs(root, runtime, stock)
            verify_framework_probe(build)
            tree = base / "tree"
            shutil.copytree(runtime, tree, symlinks=True)
            stage_stock_inputs(stock, runtime, tree)
            original = sha256(stock / "data/local/tmp/property-values.prop")
            evidence = stage_framework_inputs(root, runtime, stock, tree)
            self.assertEqual((tree / "etc").readlink(), Path("system/etc"))
            self.assertEqual(evidence["original_properties_sha256"], original)
            self.assertEqual(evidence["fixture_overrides"]["dalvik.vm.usejit"], {"original": "true", "value": "false"})
            self.assertEqual(sha256(stock / "data/local/tmp/property-values.prop"), original)
            (tree / PROGRAM).write_bytes(b"other executable")
            with self.assertRaisesRegex(ValueError, "replace"):
                stage_framework_inputs(root, runtime, stock, tree)
            (root / PROGRAM).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "changed"):
                verify_framework_inputs(root, runtime, stock)
            (build / "classes.dex").write_bytes(b"not DEX")
            with self.assertRaises(ValueError):
                verify_framework_probe(build)

    def test_evidence_requires_every_java_check_and_normal_exit(self):
        text = successful_log()
        self.assertEqual(analyze_framework(text.encode())["result"], "PASS")
        lines = text.strip().splitlines()
        for index in range(len(lines)):
            with self.subTest(missing=lines[index]):
                self.assertEqual(analyze_framework("\n".join(lines[:index] + lines[index + 1:]).encode())["result"], "INCOMPLETE")
        for broken in [text.replace("PROGRAM_EXIT 0", "PROGRAM_EXIT 137"), text + "SCARLET_FRAMEWORK_OK\n",
                       text.replace("PARCEL_BINDER_OK", "FAIL"), text.replace("LOOPER_OK", "BEGIN")]:
            self.assertEqual(analyze_framework(broken.encode())["result"], "INCOMPLETE")

    def test_runner_rejects_partial_success_and_timeout_without_preload(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            runtime, stock, root, build = fixture(base)
            kernel, firmware = base / "kernel", base / "firmware"
            kernel.write_bytes(b"kernel"); firmware.write_bytes(b"firmware")
            for variant in ("complete", "missing", "timeout"):
                output = base / variant
                def compile(command, **kwargs):
                    if command[0] == "cargo":
                        self.assertEqual(kwargs["env"]["SCARLET_ANDROID_FRAMEWORK"], "1")
                        self.assertNotIn("LD_PRELOAD", kwargs["env"]["SCARLET_ART_ENV"])
                        target = output / "target/aarch64-unknown-scarlet/release"
                        target.mkdir(parents=True, exist_ok=True)
                        for name in ("android-direct-init", "scarlet-ld"):
                            (target / name).write_bytes(b"simulated launcher")
                def guest(command, destination, timeout):
                    self.assertEqual(command[command.index("-accel") + 1], "hvf")
                    text = successful_log()
                    if variant == "missing":
                        text = text.replace("SCARLET_FRAMEWORK_LOOPER_OK\n", "")
                    (destination / "serial.log").write_text(text)
                    return {"result": "timeout" if variant == "timeout" else "PASS"}
                args = ["run-direct.py", "--kernel", str(kernel), "--framework-probe", str(build), "--framework-inputs", str(root),
                        "--runtime-root", str(runtime), "--stock-inputs", str(stock), "--accel", "hvf", "--output", str(output)]
                with patch("sys.argv", args), patch.object(runner.smoke, "firmware", return_value=firmware), \
                     patch.object(runner.subprocess, "run", side_effect=compile), patch.object(runner.smoke, "run_guest", side_effect=guest):
                    self.assertEqual(runner.main(), 0 if variant == "complete" else 1)
                result = json.loads((output / "result.json").read_text())
                self.assertEqual(result["result"] == "PASS", variant == "complete")
                if variant == "timeout":
                    self.assertEqual(result["result"], "timeout")
                self.assertEqual(json.loads((output / "program-launch.json").read_text())["argv"][0], "/system/bin/app_process64")
                self.assertEqual(json.loads((output / "inputs.json").read_text())["framework_manifest_sha256"], sha256(root / "framework-inputs.json"))


if __name__ == "__main__":
    unittest.main()
