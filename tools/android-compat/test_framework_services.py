"""Host-only evidence and launch validation for the real Java service fixture."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from art_inputs import sha256
from evidence import analyze_framework_services
from framework_inputs import SOURCE, verify_framework_probe
from test_framework import fixture, put
from test_art_runner import runner


def successful_log():
    return "\n" + "\n".join([
        "SCARLET_SERVICES_CONFIGURED", "SCARLET_ANDROID_STOCK_START properties", "SCARLET_PROPERTY_SNAPSHOT_OK 219",
        "SCARLET_ANDROID_STOCK_EXIT properties 0", "SCARLET_ANDROID_STOCK_START linkerconfig",
        "SCARLET_ANDROID_STOCK_EXIT linkerconfig 0", "SCARLET_ANDROID_STOCK_OK",
    ] + ["SCARLET_FRAMEWORK_SERVICES_" + event for event in [
        "SPAWN manager 2", "SPAWN owner 3", "SPAWN client 4", "SPAWN denied 5",
        "PEER owner", "REGISTERED", "PEER client", "PEER denied", "SERVER_CALL_OK", "CLIENT_CALL_OK",
        "SERVER_ONEWAY_OK", "CLIENT_CALLBACKS_OK", "DENIED_OK", "DONE denied", "OWNER_EXITING", "DONE owner", "EXIT owner 0", "DEATH_OK",
        "DONE client", "EXIT client 0", "EXIT denied 0", "EXIT manager 137", "OK"]]) + "\n"


class FrameworkServicesTest(unittest.TestCase):
    def test_evidence_requires_labeled_peers_rpc_death_and_reaped_processes(self):
        text = successful_log()
        self.assertEqual(analyze_framework_services(text.encode())["result"], "PASS")
        lines = text.strip().splitlines()
        for index in range(len(lines)):
            with self.subTest(missing=lines[index]):
                broken = "\n".join(lines[:index] + lines[index + 1:])
                self.assertEqual(analyze_framework_services(broken.encode())["result"], "INCOMPLETE")
        for broken in [text.replace("EXIT owner 0", "EXIT owner 137"), text.replace("SPAWN denied 5", "SPAWN denied 4"),
                       text.replace("CLIENT_CALL_OK", "DEATH_OK"), text + "SCARLET_FRAMEWORK_SERVICES_OK\n",
                       text.replace("EXIT manager 137", "EXIT manager 0")]:
            self.assertEqual(analyze_framework_services(broken.encode())["result"], "INCOMPLETE")

    def test_runner_binds_java_source_policy_and_manager_and_preserves_timeouts(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            runtime, stock, root, build = fixture(base)
            with self.assertRaises(ValueError):
                verify_framework_probe(build, services=True)
            metadata = json.loads((build / "build.json").read_text())
            metadata["framework_service_sha256"] = sha256(SOURCE / "framework/ScarletFrameworkService.java")
            (build / "build.json").write_text(json.dumps(metadata))
            library = put(runtime, "system/lib64/libselinux.so", b"simulated library")
            record = json.loads((runtime / "art-runtime.json").read_text())
            record["files"]["system/lib64/libselinux.so"] = sha256(library)
            (runtime / "art-runtime.json").write_text(json.dumps(record))
            record = json.loads((stock / "stock-inputs.json").read_text())
            record["base_manifest_sha256"] = sha256(runtime / "art-runtime.json")
            (stock / "stock-inputs.json").write_text(json.dumps(record))
            record = json.loads((root / "framework-inputs.json").read_text())
            record.update(base_manifest_sha256=sha256(runtime / "art-runtime.json"), stock_manifest_sha256=sha256(stock / "stock-inputs.json"))
            (root / "framework-inputs.json").write_text(json.dumps(record))
            services = base / "services"
            program = put(services, "system/bin/servicemanager", b"simulated manager")
            (services / "service-inputs.json").write_text(json.dumps({
                "result": "PREPARED", "base_manifest_sha256": sha256(runtime / "art-runtime.json"),
                "files": {"system/bin/servicemanager": sha256(program)}, "runtime_libraries": {"system/lib64/libselinux.so": sha256(library)}}))
            kernel, firmware = base / "kernel", base / "firmware"
            kernel.write_bytes(b"kernel"); firmware.write_bytes(b"firmware")
            for variant in ("complete", "missing", "timeout"):
                output = base / variant
                def compile(command, **kwargs):
                    if command[0] == "cargo":
                        environment = kwargs["env"]
                        self.assertEqual(environment["SCARLET_ANDROID_FRAMEWORK_SERVICES"], "1")
                        self.assertTrue(environment["SCARLET_ART_ARGS"].endswith("ScarletFrameworkService"))
                        self.assertNotIn("LD_PRELOAD", environment["SCARLET_ART_ENV"])
                        target = output / "target/aarch64-unknown-scarlet/release"
                        target.mkdir(parents=True, exist_ok=True)
                        for name in ("android-direct-init", "scarlet-ld"):
                            (target / name).write_bytes(b"simulated launcher")
                def guest(command, destination, timeout):
                    self.assertEqual(command[command.index("-accel") + 1], "hvf")
                    text = successful_log()
                    if variant == "missing":
                        text = text.replace("SCARLET_FRAMEWORK_SERVICES_DEATH_OK\n", "")
                    self.assertIsNotNone(runner.smoke.SUCCESS.search(text.encode()))
                    (destination / "serial.log").write_text(text)
                    return {"result": "timeout" if variant == "timeout" else "PASS"}
                args = ["run-direct.py", "--kernel", str(kernel), "--framework-services", str(build), "--framework-inputs", str(root),
                        "--service-inputs", str(services), "--runtime-root", str(runtime), "--stock-inputs", str(stock),
                        "--accel", "hvf", "--output", str(output)]
                with patch("sys.argv", args), patch.object(runner.smoke, "firmware", return_value=firmware), \
                     patch.object(runner.subprocess, "run", side_effect=compile), patch.object(runner.smoke, "run_guest", side_effect=guest):
                    self.assertEqual(runner.main(), 0 if variant == "complete" else 1)
                result = json.loads((output / "result.json").read_text())
                self.assertEqual(result["result"] == "PASS", variant == "complete")
                if variant == "timeout":
                    self.assertEqual(result["result"], "timeout")
                inputs = json.loads((output / "inputs.json").read_text())
                self.assertEqual(inputs["service_manifest_sha256"], sha256(services / "service-inputs.json"))
                self.assertEqual(inputs["framework_fixture_sha256"]["framework_services.rs"], sha256(SOURCE / "init/src/framework_services.rs"))


if __name__ == "__main__":
    unittest.main()
