import importlib.util
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest


DIRECTORY = Path(__file__).resolve().parent
sys.path.insert(0, str(DIRECTORY))
SPEC = importlib.util.spec_from_file_location("native_rustc_run_qemu", DIRECTORY / "run-qemu.py")
assert SPEC and SPEC.loader
RUN_QEMU = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUN_QEMU)


class StagingLinkTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        (self.root / "bin").mkdir()
        (self.root / "lib").mkdir()
        (self.root / "lib/driver.so").write_text("driver")

    def tearDown(self):
        self.temporary.cleanup()

    def test_relative_file_link_within_staging_is_allowed(self):
        (self.root / "bin/driver.so").symlink_to("../lib/driver.so")
        RUN_QEMU.validate_staging_links(self.root)

    def test_absolute_link_is_rejected(self):
        outside = self.root.parent / (self.root.name + "-outside")
        outside.write_text("outside")
        self.addCleanup(outside.unlink)
        (self.root / "bin/driver.so").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "must be relative"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_relative_link_escaping_staging_is_rejected(self):
        outside = self.root.parent / (self.root.name + "-outside")
        outside.write_text("outside")
        self.addCleanup(outside.unlink)
        (self.root / "bin/driver.so").symlink_to(Path("../..") / outside.name)
        with self.assertRaisesRegex(ValueError, "escapes its root"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_relative_link_to_staging_root_is_rejected(self):
        (self.root / "bin/root").symlink_to("../..")
        with self.assertRaisesRegex(ValueError, "escapes its root"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_broken_link_is_rejected(self):
        (self.root / "bin/driver.so").symlink_to("../lib/missing.so")
        with self.assertRaisesRegex(ValueError, "invalid staged symlink"):
            RUN_QEMU.validate_staging_links(self.root)

    def test_old_runtime_copy_in_loader_search_path_is_rejected(self):
        name = "librustc_driver-fixture.so"
        (self.root / "bin" / name).write_bytes(b"old TLS layout")
        (self.root / "lib" / name).write_bytes(b"new TLS layout")
        with self.assertRaisesRegex(ValueError, "conflicting Rust runtime copies"):
            RUN_QEMU.validate_runtime_copies(self.root)
        (self.root / "bin" / name).write_bytes(b"new TLS layout")
        RUN_QEMU.validate_runtime_copies(self.root)


class CStartupEvidenceTests(unittest.TestCase):
    def test_marker_cannot_mask_missing_or_failed_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with self.assertRaisesRegex(ValueError, "evidence is missing"):
                RUN_QEMU.validate_c_startup_evidence(output)
            (output / "C_STARTUP_PASS").touch()
            with self.assertRaises(FileNotFoundError):
                RUN_QEMU.validate_c_startup_evidence(output)
            for status in ("exit=Some(0) ", "exit=Some(139) ", "exit=None "):
                (output / "c-startup.status").write_text(status)
                with self.assertRaisesRegex(ValueError, "required status 43"):
                    RUN_QEMU.validate_c_startup_evidence(output)
            (output / "c-startup.status").write_text("exit=Some(43) elapsed_ms=10 expected_exit=43\n")
            RUN_QEMU.validate_c_startup_evidence(output)


class SerialDiagnosticTests(unittest.TestCase):
    def test_panic_diagnostic_is_drained_and_late_success_cannot_override_failure(self):
        script = (
            "import sys,time; "
            "sys.stdout.write('panicked at'); sys.stdout.flush(); time.sleep(0.03); "
            "sys.stdout.write(' fixture.rs:42: assertion failed\\nSCARLET_LOADER_SMOKE_OK\\n'); "
            "sys.stdout.flush(); time.sleep(1)"
        )
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            captured = io.TextIOWrapper(io.BytesIO(), encoding="utf-8")
            with contextlib.redirect_stdout(captured):
                result = RUN_QEMU.smoke.run_guest([sys.executable, "-c", script], output, 3)
            self.assertEqual(result["result"], "guest panic")
            self.assertIn(b"fixture.rs:42: assertion failed", (output / "serial.log").read_bytes())


if __name__ == "__main__":
    unittest.main()
