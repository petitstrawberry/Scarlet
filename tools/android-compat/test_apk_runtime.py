"""APK evidence must include parsing, verified signatures, resources and payload execution."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from apk_inputs import APK, FRAMEWORK_RESOURCES, SOURCE, stage_apk_probe, verify_apk_probe
from art_inputs import sha256
from evidence import analyze_apk_runtime
from test_art_runner import runner
from test_framework import fixture, put


def successful_log():
    return "\n" + "\n".join([
        "SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK 219",
        "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
        "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK",
        "SCARLET_ANDROID_PROGRAM_START", "SCARLET_APK_RUNTIME_BEGIN", "SCARLET_APK_RUNTIME_PACKAGE_LITE_OK",
        "SCARLET_APK_RUNTIME_SIGNATURE_OK", "SCARLET_APK_RUNTIME_TAMPER_REJECTED", "SCARLET_APK_RUNTIME_RESOURCES_OK", "SCARLET_APK_RUNTIME_DEX_OK",
        "SCARLET_APK_RUNTIME_OK", "SCARLET_ANDROID_PROGRAM_EXIT 0", "SCARLET_ANDROID_PROGRAM_DONE"]) + "\n"


def apk_fixture(base):
    runtime, stock, root, build = fixture(base)
    resources = put(root, FRAMEWORK_RESOURCES, b"simulated image resource package")
    framework = json.loads((root / "framework-inputs.json").read_text())
    framework["files"][FRAMEWORK_RESOURCES] = sha256(resources)
    (root / "framework-inputs.json").write_text(json.dumps(framework))
    with zipfile.ZipFile(build / "scarlet-compat.apk", "w") as archive:
        for name in ("classes.dex", "AndroidManifest.xml", "resources.arsc", "assets/probe.txt"):
            archive.writestr(name, b"simulated fixture data")
    (build / "scarlet-tampered.apk").write_bytes(b"simulated invalid signed APK")
    metadata = json.loads((build / "build.json").read_text())
    metadata.update(main_class="ScarletApkProbe", package="dev.scarlet.compat",
                    tampered_sha256=sha256(build / "scarlet-tampered.apk"),
                    source_sha256=sha256(SOURCE / "framework/ScarletApkProbe.java"), sha256=sha256(build / "scarlet-compat.apk"),
                    sources={str(p.relative_to(SOURCE)): sha256(p) for p in (SOURCE / "apk").rglob("*") if p.is_file()})
    (build / "build.json").write_text(json.dumps(metadata))
    return runtime, stock, root, build, framework, metadata


class ApkRuntimeTest(unittest.TestCase):
    def test_evidence_requires_every_checkpoint_and_never_claims_activity_launch(self):
        text = successful_log()
        evidence = analyze_apk_runtime(text.encode())
        self.assertEqual(evidence["result"], "PASS")
        self.assertEqual(evidence["activity_launch"], "not tested")
        lines = text.strip().splitlines()
        for index in range(len(lines)):
            with self.subTest(missing=lines[index]):
                self.assertEqual(analyze_apk_runtime("\n".join(lines[:index] + lines[index + 1:]).encode())["result"], "INCOMPLETE")
        for broken in [text.replace("PROGRAM_EXIT 0", "PROGRAM_EXIT 137"), text + "SCARLET_APK_RUNTIME_OK\n",
                       text.replace("SIGNATURE_OK", "RESOURCES_OK")]:
            self.assertEqual(analyze_apk_runtime(broken.encode())["result"], "INCOMPLETE")

    def test_inputs_bind_apk_source_dex_resources_and_staging_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            runtime, stock, root, build, framework, metadata = apk_fixture(base)
            verify_apk_probe(build, framework)
            tree = base / "tree"
            stage_apk_probe(build, tree, metadata["sha256"], metadata["tampered_sha256"])
            self.assertEqual(sha256(tree / APK), metadata["sha256"])
            with self.assertRaises(ValueError):
                stage_apk_probe(build, tree, metadata["sha256"], metadata["tampered_sha256"])
            with self.assertRaises(ValueError):
                stage_apk_probe(build, base / "bad", "changed", "changed")
            without_resources = {"files": {}}
            with self.assertRaisesRegex(ValueError, "framework-res"):
                verify_apk_probe(build, without_resources)
            for field in ("sha256", "tampered_sha256", "dex_sha256", "source_sha256", "sources"):
                (build / "build.json").write_text(json.dumps({**metadata, field: None}))
                with self.subTest(field=field), self.assertRaises(ValueError):
                    verify_apk_probe(build, framework)
            (build / "build.json").write_text(json.dumps(metadata))
            (build / "scarlet-compat.apk").write_bytes(b"changed APK")
            with self.assertRaises(ValueError):
                verify_apk_probe(build, framework)

    def test_runner_stages_apk_and_keeps_missing_checks_and_timeouts_failed(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            runtime, stock, root, build, framework, metadata = apk_fixture(base)
            kernel, firmware = base / "kernel", base / "firmware"
            kernel.write_bytes(b"kernel"); firmware.write_bytes(b"firmware")
            for variant in ("complete", "missing", "timeout"):
                output = base / variant
                def compile(command, **kwargs):
                    if command[0] == "cargo":
                        environment = kwargs["env"]
                        self.assertTrue(environment["SCARLET_ART_ARGS"].endswith("ScarletApkProbe"))
                        self.assertNotIn("LD_PRELOAD", environment["SCARLET_ART_ENV"])
                        target = output / "target/aarch64-unknown-scarlet/release"
                        target.mkdir(parents=True, exist_ok=True)
                        for name in ("android-direct-init", "scarlet-ld"):
                            (target / name).write_bytes(b"simulated launcher")
                def guest(command, destination, timeout):
                    self.assertEqual(command[command.index("-accel") + 1], "hvf")
                    staged = output / "staging/systems/android-aarch64" / APK
                    self.assertEqual(sha256(staged), metadata["sha256"])
                    text = successful_log()
                    if variant == "missing":
                        text = text.replace("SCARLET_APK_RUNTIME_SIGNATURE_OK\n", "")
                    (destination / "serial.log").write_text(text)
                    return {"result": "timeout" if variant == "timeout" else "PASS"}
                args = ["run-direct.py", "--kernel", str(kernel), "--apk-probe", str(build), "--framework-inputs", str(root),
                        "--runtime-root", str(runtime), "--stock-inputs", str(stock), "--accel", "hvf", "--output", str(output)]
                with patch("sys.argv", args), patch.object(runner.smoke, "firmware", return_value=firmware), \
                     patch.object(runner.subprocess, "run", side_effect=compile), patch.object(runner.smoke, "run_guest", side_effect=guest):
                    self.assertEqual(runner.main(), 0 if variant == "complete" else 1)
                result = json.loads((output / "result.json").read_text())
                self.assertEqual(result["result"] == "PASS", variant == "complete")
                self.assertEqual(result["activity_launch"], "not tested")
                if variant == "timeout":
                    self.assertEqual(result["result"], "timeout")
                self.assertEqual(json.loads((output / "inputs.json").read_text())["apk_sha256"], metadata["sha256"])


if __name__ == "__main__":
    unittest.main()
