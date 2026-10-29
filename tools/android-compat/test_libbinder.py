import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from art_inputs import sha256
from evidence import analyze_libbinder
from libbinder_inputs import LIBRARIES, SOURCE, extract_headers, verify_libbinder_build
from test_art_runner import runner, runtime_fixture, stock_fixture


def successful_log():
    lines = ["SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK 216",
             "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
             "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK",
             "SCARLET_ANDROID_PROGRAM_START", "SCARLET_LIBBINDER_BEGIN",
             "SCARLET_LIBBINDER_RUNTIME /system/lib64/libbinder.so", "SCARLET_LIBBINDER_SERVER_READY",
             "SCARLET_LIBBINDER_CONTEXT_OK"]
    for cycle in (1, 2):
        for step in ("HANDLE_OK", "ROUNDTRIP_OK", "CALL_OK", "STATUS_OK", "STRONG_ZERO_WEAK_LIVE", "REFS_RELEASED"):
            lines.append(f"SCARLET_LIBBINDER_CYCLE {cycle} {step}")
    lines += ["SCARLET_LIBBINDER_SERVER_EXIT 0", "SCARLET_LIBBINDER_OK",
              "SCARLET_ANDROID_PROGRAM_EXIT 0", "SCARLET_ANDROID_PROGRAM_DONE"]
    return "\n" + "\n".join(lines) + "\n"


def build_fixture(base):
    runtime, build = base / "runtime", base / "build"
    runtime.mkdir()
    build.mkdir()
    metadata = runtime_fixture(runtime)
    for name in LIBRARIES:
        path = runtime / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(name.encode())
        metadata["files"][name] = sha256(path)
    (runtime / "art-runtime.json").write_text(json.dumps(metadata))
    binary = build / "libbinder-probe"
    binary.write_bytes(b"test-only simulated compiler output")
    info = {"result": "BUILT", "kind": "stock-libbinder-probe-v1",
            "runtime_manifest_sha256": sha256(runtime / "art-runtime.json"),
            "runtime_libraries": {name: sha256(runtime / name) for name in LIBRARIES},
            "source_sha256": sha256(SOURCE / "libbinder-probe.cpp"),
            "headers_manifest_sha256": sha256(SOURCE / "libbinder-headers.json"),
            "binary_sha256": sha256(binary), "needed": ["libbinder.so", "libc++.so"]}
    (build / "build.json").write_text(json.dumps(info))
    return runtime, build, info


class LibbinderTest(unittest.TestCase):
    def test_evidence_requires_every_stage_and_ordered_reference_release(self):
        complete = successful_log()
        self.assertEqual(analyze_libbinder(complete.encode())["result"], "PASS")
        lines = complete.strip().splitlines()
        for index in range(len(lines)):
            with self.subTest(missing=lines[index]):
                self.assertEqual(analyze_libbinder("\n".join(lines[:index] + lines[index + 1:]).encode())["result"], "INCOMPLETE")
        variants = [complete.replace("PROGRAM_EXIT 0", "PROGRAM_EXIT 1"),
                    complete.replace("/system/lib64/libbinder.so", "/tmp/shim.so"),
                    complete.replace("SCARLET_LIBBINDER_OK", "SCARLET_LIBBINDER_FAIL\nSCARLET_LIBBINDER_OK"),
                    complete.replace("SCARLET_LIBBINDER_OK", "SCARLET_LIBBINDER_OK\nSCARLET_LIBBINDER_OK"),
                    complete.replace("1 STRONG_ZERO_WEAK_LIVE", "1 REFS_RELEASED")]
        for data in variants:
            self.assertEqual(analyze_libbinder(data.encode())["result"], "INCOMPLETE")
        self.assertEqual(analyze_libbinder(b"SCARLET_ANDROID_PROGRAM_EXIT 0\nSCARLET_ANDROID_PROGRAM_DONE\n")["result"], "INCOMPLETE")

    def test_header_content_pin_ignores_archive_metadata_but_rejects_changed_bytes_and_paths(self):
        def archive(name="binder/test.h", payload=b"// header\n", mtime=0):
            stream = io.BytesIO()
            with tarfile.open(fileobj=stream, mode="w:gz") as tar:
                member = tarfile.TarInfo(name)
                member.size, member.mtime = len(payload), mtime
                tar.addfile(member, io.BytesIO(payload))
                link = tarfile.TarInfo("unused.h")
                link.type, link.linkname = tarfile.SYMTYPE, "../outside.h"
                tar.addfile(link)
            return stream.getvalue()
        files = {"binder/test.h": hashlib.sha256(b"// header\n").hexdigest()}
        pin = hashlib.sha256(json.dumps(files, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            for mtime in (0, 1234):
                target = base / str(mtime)
                extract_headers(archive(mtime=mtime), pin, target)
                self.assertEqual((target / "binder/test.h").read_bytes(), b"// header\n")
                self.assertFalse((target / "unused.h").exists())
            for name, payload in [("../outside.h", b"// header\n"), ("/absolute.h", b"// header\n"),
                                  ("binder/test.h", b"changed")]:
                with self.assertRaises(ValueError):
                    extract_headers(archive(name, payload), pin, base / "rejected")
                self.assertFalse((base / "rejected").exists())

    def test_build_identity_rejects_partial_or_stale_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            runtime, build, metadata = build_fixture(Path(temporary))
            binary = verify_libbinder_build(build, runtime)
            for key, value in [("result", "PREPARING"), ("binary_sha256", "wrong"),
                               ("runtime_manifest_sha256", "wrong"), ("runtime_libraries", {}),
                               ("source_sha256", "wrong"), ("headers_manifest_sha256", "wrong"),
                               ("needed", ["libbinder.so", "libc++_shared.so"])]:
                with self.subTest(key=key):
                    (build / "build.json").write_text(json.dumps(dict(metadata, **{key: value})))
                    with self.assertRaises(ValueError):
                        verify_libbinder_build(build, runtime)
            (build / "build.json").write_text(json.dumps(metadata))
            binary.write_bytes(b"changed executable")
            with self.assertRaises(ValueError):
                verify_libbinder_build(build, runtime)

    def test_runner_certifies_only_complete_rpc_evidence_and_preserves_timeout(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            runtime, build, _ = build_fixture(base)
            stock = base / "stock"
            stock_fixture(stock, runtime)
            kernel, firmware = base / "kernel", base / "firmware"
            kernel.write_bytes(b"kernel")
            firmware.write_bytes(b"firmware")
            for variant in ("complete", "missing", "timeout"):
                output = base / variant
                def compile(command, **kwargs):
                    if command[0] == "cargo":
                        self.assertEqual(kwargs["env"]["SCARLET_ANDROID_PROGRAM"], "1")
                        target = output / "target/aarch64-unknown-scarlet/release"
                        target.mkdir(parents=True, exist_ok=True)
                        for name in ("android-direct-init", "scarlet-ld"):
                            (target / name).write_bytes(b"simulated launcher")
                def guest(command, destination, timeout):
                    self.assertEqual(command[command.index("-accel") + 1], "hvf")
                    data = successful_log()
                    if variant == "missing":
                        data = data.replace("SCARLET_LIBBINDER_CYCLE 1 CALL_OK\n", "")
                    (destination / "serial.log").write_text(data)
                    return {"result": "timeout" if variant == "timeout" else "PASS"}
                args = ["run-direct.py", "--kernel", str(kernel), "--libbinder-probe", str(build),
                        "--runtime-root", str(runtime), "--stock-inputs", str(stock),
                        "--accel", "hvf", "--output", str(output)]
                with patch("sys.argv", args), patch.object(runner.smoke, "firmware", return_value=firmware), \
                        patch.object(runner.subprocess, "run", side_effect=compile), \
                        patch.object(runner.smoke, "run_guest", side_effect=guest):
                    self.assertEqual(runner.main(), 0 if variant == "complete" else 1)
                result = json.loads((output / "result.json").read_text())
                self.assertEqual(result["result"] == "PASS", variant == "complete")
                if variant == "timeout":
                    self.assertEqual(result["result"], "timeout")
                self.assertEqual(result["servicemanager"], "not tested")
                self.assertEqual((output / "libbinder-build.json").read_bytes(), (build / "build.json").read_bytes())
                inputs = json.loads((output / "inputs.json").read_text())
                self.assertEqual(inputs["libbinder_build_sha256"], sha256(build / "build.json"))


if __name__ == "__main__":
    unittest.main()
