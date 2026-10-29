import unittest
import json
import tempfile
from pathlib import Path
from art_inputs import sha256
from zygote_inputs import verify_build, SOURCE
from zygote_inputs import analyze, launch, header


class ZygoteEvidenceTest(unittest.TestCase):
    def test_requires_socket_responses_matching_pid_and_owned_daemon_cleanup(self):
        lines = ["SCARLET_ANDROID_STOCK_OK", "SCARLET_ZYGOTE_PROBE_SPAWN 8",
                 "SCARLET_ZYGOTE_PROBE_ABI_OK arm64-v8a", "SCARLET_ZYGOTE_PROBE_PID_OK 8",
                 "SCARLET_ZYGOTE_PROBE_SHIELD_OK", "SCARLET_ZYGOTE_PROBE_REAP 9", "SCARLET_ZYGOTE_PROBE_OK",
                 "SCARLET_ANDROID_PROGRAM_EXIT 0", "SCARLET_ANDROID_PROGRAM_DONE"]
        good = "\n".join(lines).encode()
        self.assertEqual(analyze(good)["result"], "PASS")
        for line in lines:
            self.assertEqual(analyze(good.replace(line.encode(), b""))["result"], "INCOMPLETE")
        for before, after in [(b"PID_OK 8", b"PID_OK 9"), (b"REAP 9", b"REAP 0"),
                              (b"PROGRAM_EXIT 0", b"PROGRAM_EXIT 1")]:
            self.assertEqual(analyze(good.replace(before, after))["result"], "INCOMPLETE")
        self.assertEqual(analyze(good+b"\nSCARLET_ZYGOTE_PROBE_OK")["result"], "INCOMPLETE")

    def test_launch_uses_verified_classpath_without_preload_or_application_mode(self):
        metadata = {"boot_class_path": ["/apex/a.jar", "/system/framework/b.jar"]}
        value = launch(metadata)
        self.assertIn("--zygote", value["argv"])
        self.assertNotIn("--application", value["argv"])
        self.assertIn("-Xzygote", value["argv"])
        self.assertEqual(value["argv"][8], "/system/bin")
        self.assertIn("BOOTCLASSPATH=/apex/a.jar:/system/framework/b.jar", value["env"])
        self.assertFalse(any(e.startswith(("LD_PRELOAD=", "CLASSPATH=")) for e in value["env"]))
        self.assertIn('"--zygote"', header(metadata))

    def test_build_rejects_a_stale_launch_or_changed_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = {"boot_class_path": ["/system/framework/test.jar"]}
            (root / "framework-inputs.json").write_text("{}")
            (root / "zygote-probe").write_bytes(b"test binary")
            (root / "zygote-config.h").write_text(header(metadata))
            record = {"kind": "stock-zygote-control-v2", "source_sha256": sha256(SOURCE / "zygote-probe.c"),
                      "framework_sha256": sha256(root / "framework-inputs.json"),
                      "program_sha256": sha256(root / "zygote-probe")}
            (root / "build.json").write_text(json.dumps(record))
            self.assertEqual(verify_build(root, metadata, root), root / "zygote-probe")
            (root / "zygote-config.h").write_text(header(metadata).replace('"-Xzygote",', ""))
            with self.assertRaises(ValueError): verify_build(root, metadata, root)
            (root / "zygote-config.h").write_text(header(metadata))
            (root / "zygote-probe").write_bytes(b"different binary")
            with self.assertRaises(ValueError): verify_build(root, metadata, root)


class ZygoteRunnerTest(unittest.TestCase):
    def test_runner_records_and_applies_shield_for_zygote_and_program_diagnostics(self):
        from unittest.mock import patch
        from test_framework import fixture
        from test_art_runner import runner
        for diagnostic in (False, True):
            with self.subTest(diagnostic=diagnostic), tempfile.TemporaryDirectory() as temporary:
                base = Path(temporary)
                runtime, stock, framework, build = fixture(base)
                metadata = json.loads((framework / "framework-inputs.json").read_text())
                binary = build / "zygote-probe"
                binary.write_bytes(b"simulated probe")
                (build / "zygote-config.h").write_text(header(metadata))
                (build / "build.json").write_text(json.dumps({
                    "kind": "stock-zygote-control-v2", "source_sha256": sha256(SOURCE / "zygote-probe.c"),
                    "framework_sha256": sha256(framework / "framework-inputs.json"),
                    "program_sha256": sha256(binary)}))
                kernel, firmware, output = base / "kernel", base / "firmware", base / "run"
                kernel.write_bytes(b"kernel"); firmware.write_bytes(b"firmware")
                def compile(command, **kwargs):
                    if command[0] == "cargo":
                        self.assertEqual(kwargs["env"]["SCARLET_ANDROID_SHIELD"], "1")
                        target = output / "target/aarch64-unknown-scarlet/release"
                        target.mkdir(parents=True, exist_ok=True)
                        for name in ("android-direct-init", "scarlet-ld"):
                            (target / name).write_bytes(b"simulated launcher")
                def guest(command, destination, timeout):
                    self.assertEqual(command[command.index("-accel")+1], "hvf")
                    lines = ["SCARLET_SHIELD_CONFIGURED", "SCARLET_ANDROID_STOCK_START properties",
                        "SCARLET_PROPERTY_SNAPSHOT_OK", "SCARLET_ANDROID_STOCK_EXIT properties 0",
                        "SCARLET_ANDROID_STOCK_START linkerconfig", "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0",
                        "SCARLET_ANDROID_STOCK_OK", "SCARLET_ANDROID_PROGRAM_START",
                        "SCARLET_ZYGOTE_PROBE_SPAWN 8", "SCARLET_ZYGOTE_PROBE_ABI_OK arm64-v8a",
                        "SCARLET_ZYGOTE_PROBE_PID_OK 8", "SCARLET_ZYGOTE_PROBE_SHIELD_OK",
                        "SCARLET_ZYGOTE_PROBE_REAP 9", "SCARLET_ZYGOTE_PROBE_OK",
                        "SCARLET_ANDROID_PROGRAM_EXIT 0", "SCARLET_ANDROID_PROGRAM_DONE"]
                    (destination / "serial.log").write_text("\n" + "\n".join(lines) + "\n")
                    return {"result": "PASS"}
                mode = ["--program", str(binary), "--shield-fixture"] if diagnostic else [
                    "--zygote-probe", str(build), "--framework-inputs", str(framework)]
                args = ["run-direct.py", "--kernel", str(kernel), *mode, "--runtime-root", str(runtime),
                    "--stock-inputs", str(stock), "--accel", "hvf", "--output", str(output)]
                with patch("sys.argv", args), patch.object(runner.smoke, "firmware", return_value=firmware), \
                        patch.object(runner.subprocess, "run", side_effect=compile), \
                        patch.object(runner.smoke, "run_guest", side_effect=guest):
                    self.assertEqual(runner.main(), 0)
                inputs = json.loads((output / "inputs.json").read_text())
                self.assertEqual(inputs["shield_fixture_sha256"], sha256(SOURCE / "init/src/shield.rs"))
                self.assertIsNone(inputs["selinux_build_sha256"])
