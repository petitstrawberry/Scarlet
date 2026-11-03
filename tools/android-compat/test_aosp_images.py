import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import aosp_images
from art_inputs import sha256


class CompleteImagesTest(unittest.TestCase):
    def test_preserves_every_userspace_member_and_detects_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "sdk.zip"
            payloads = {name: name.encode() for name in aosp_images.REQUIRED}
            payloads["data/unselected-service/config"] = b"keep the complete payload"
            payloads["NOTICE.txt"] = b"license"
            with zipfile.ZipFile(archive, "w") as target:
                for name, value in payloads.items():
                    target.writestr("arm64-v8a/" + name, value)
                target.writestr("arm64-v8a/kernel-ranchu", b"not Scarlet")
            with patch.object(aosp_images, "SDK_SHA256", sha256(archive)):
                output = root / "inputs"
                result = aosp_images.prepare(archive, output)
                self.assertEqual(set(result["files"]), set(payloads))
                for name, value in payloads.items():
                    self.assertEqual((output / name).read_bytes(), value)
                self.assertEqual(result["boot_validation"], "not run")
                self.assertEqual(aosp_images.verify(output), result)
                with self.assertRaises(FileExistsError):
                    aosp_images.prepare(archive, output)
                (output / "extra").write_bytes(b"extra")
                with self.assertRaisesRegex(ValueError, "unexpected"):
                    aosp_images.verify(output)
                (output / "extra").unlink()
                image = output / "system.img"
                image.chmod(0o644)
                image.write_bytes(b"modified")
                with self.assertRaisesRegex(ValueError, "changed"):
                    aosp_images.verify(output)

    def test_sparse_copy_keeps_zero_runs_and_tail(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "image"
            data = bytes(1024 * 1024) + b"content" + bytes(1024 * 1024)
            record = aosp_images.copy_sparse(io.BytesIO(data), target, len(data))
            self.assertEqual(target.read_bytes(), data)
            self.assertEqual(record["sha256"], sha256(target))

    def test_rejects_wrong_source_before_creating_output(self):
        with tempfile.TemporaryDirectory() as directory:
            source, output = Path(directory) / "wrong.zip", Path(directory) / "out"
            source.write_bytes(b"wrong")
            with self.assertRaisesRegex(ValueError, "pinned"):
                aosp_images.prepare(source, output)
            self.assertFalse(output.exists())

    def test_rejects_archive_paths_outside_input_tree(self):
        for name in ["elsewhere/file", "arm64-v8a/../outside", "arm64-v8a//absolute",
                     "arm64-v8a/a/./b", "arm64-v8a/", "arm64-v8a/a\nb"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                aosp_images.member_path(name)


if __name__ == "__main__":
    unittest.main()
