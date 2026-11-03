import hashlib
import json
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest

from art_inputs import SDK_SHA256, sha256
from aosp_inventory import Image
from stock_art import PARTITIONS
from stock_packages import MANIFEST, install_image, stage, verify


@unittest.skipUnless(shutil.which("debugfs") and shutil.which("mkfs.ext2"), "requires e2fsprogs")
class StockPackageImageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="stock-package-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.inputs = self.root / "inputs"
        (self.inputs / "objects").mkdir(parents=True)
        nodes = {"system/app": {"mode": stat.S_IFDIR | 0o755, "uid": 0, "gid": 2000, "size": 0}}
        for name, contents in [("Case.apk", b"UPPER CASE"), ("case.apk", b"lower case")]:
            digest = hashlib.sha256(contents).hexdigest()
            (self.inputs / "objects" / digest).write_bytes(contents)
            nodes["system/app/" + name] = {"mode": stat.S_IFREG | 0o640, "uid": 1234,
                                           "gid": 5678, "size": len(contents), "sha256": digest}
        nodes["system/app/alias.apk"] = {"mode": stat.S_IFLNK | 0o777, "uid": 0, "gid": 0,
                                          "size": 8, "target": "Case.apk"}
        metadata = {"kind": "scarlet-stock-packages-v1", "sdk_sha256": SDK_SHA256,
                    "result": "PREPARED", "partitions": dict.fromkeys(PARTITIONS, "0" * 64),
                    "trees": ["system/app"], "nodes": nodes}
        (self.inputs / MANIFEST).write_text(json.dumps(metadata))
        self.staging = self.root / "staging"
        (self.staging / "systems/android-aarch64").mkdir(parents=True)
        self.disk = self.root / "rootfs.ext2"

    def make_disk(self):
        with self.disk.open("wb") as disk: disk.truncate(32 * 1024**2)
        subprocess.run(["mkfs.ext2", "-q", "-F", "-d", str(self.staging), str(self.disk)], check=True)

    def test_case_distinct_payloads_and_guest_inode_identities_survive(self):
        stage(self.inputs, self.staging / "systems/android-aarch64")
        self.make_disk()
        proof = install_image(self.inputs, self.disk, "/systems/android-aarch64")
        self.assertEqual(proof["nodes"], 4)
        image = Image(self.disk, "debugfs")
        base = "/systems/android-aarch64/system/app/"
        self.assertEqual(image.command("cat " + base + "Case.apk"), b"UPPER CASE")
        self.assertEqual(image.command("cat " + base + "case.apk"), b"lower case")
        self.assertIn(b'Fast link dest: "Case.apk"', image.command("stat " + base + "alias.apk"))
        subprocess.run(["e2fsck", "-fn", str(self.disk)], check=True, capture_output=True)

    def test_conflicting_existing_bytes_leave_the_disk_untouched(self):
        app = self.staging / "systems/android-aarch64/system/app"
        app.mkdir(parents=True)
        (app / "Case.apk").write_bytes(b"local modified APK")
        with self.assertRaisesRegex(ValueError, "content conflict"):
            stage(self.inputs, self.staging / "systems/android-aarch64")
        self.make_disk()
        before = sha256(self.disk)
        with self.assertRaisesRegex(ValueError, "content conflict"):
            install_image(self.inputs, self.disk, "/systems/android-aarch64")
        self.assertEqual(before, sha256(self.disk))

    def test_changed_object_is_rejected_before_staging(self):
        next((self.inputs / "objects").iterdir()).write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "changed stock package"):
            verify(self.inputs)


if __name__ == "__main__":
    unittest.main()
