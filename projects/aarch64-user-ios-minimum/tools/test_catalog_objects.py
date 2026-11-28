"""Regression checks for explicit signed-catalog exclusions (no guest builds)."""

import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest


class CatalogPolicyTests(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory()
        self.addCleanup(self.work.cleanup)
        self.root = Path(self.work.name) / "rootfs"
        self.root.mkdir()
        self.binary = self.root / "fixed"
        header = bytearray(64)
        header[:8] = b"\x7fELF\x02\x01\x01S"
        struct.pack_into("<HH", header, 16, 2, 183)
        self.binary.write_bytes(header)
        self.policy = self.root.parent / "exclusions.json"
        self.policy.write_text(json.dumps({"rootfs/fixed": {
            "sha256": hashlib.sha256(header).hexdigest(),
            "reason": "Fixture fixed-address prebuilt",
        }}))

    def run_catalog(self, policy=True):
        command = [sys.executable, str(Path(__file__).with_name("catalog_objects.py"))]
        if policy:
            command += ["--exclusions", str(self.policy)]
        return subprocess.run(command + [str(self.root)], capture_output=True)

    def test_fixed_binary_is_rejected_by_default(self):
        result = self.run_catalog(policy=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Expected a Scarlet PIE or DSO", result.stderr)

    def test_exact_exclusion_is_reported_and_not_registered(self):
        result = self.run_catalog()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"Excluded from signed catalog: rootfs/fixed", result.stderr)

    def test_changed_exclusion_fails(self):
        self.binary.write_bytes(self.binary.read_bytes() + b"changed")
        result = self.run_catalog()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Catalog exclusion needs review", result.stderr)

    def test_missing_exclusion_fails(self):
        self.binary.unlink()
        result = self.run_catalog()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Catalog exclusions are absent", result.stderr)

    def test_exclusion_does_not_allow_other_fixed_binaries(self):
        (self.root / "other").write_bytes(self.binary.read_bytes())
        result = self.run_catalog()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Expected a Scarlet PIE or DSO", result.stderr)


if __name__ == "__main__":
    unittest.main()
