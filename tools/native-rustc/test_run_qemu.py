import importlib.util
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


if __name__ == "__main__":
    unittest.main()
