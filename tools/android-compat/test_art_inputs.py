import hashlib
import io
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zipfile
import zlib

from art_inputs import Ext4, copy_exact, elf_needed, extract_system, quote_debugfs, unpack_apex, validate_dex
from evidence import analyze_art


def zip_bytes(entries):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w") as archive:
        for name, data in entries.items():
            archive.writestr(name, data)
    return stream.getvalue()


class ArtInputTest(unittest.TestCase):
    def test_wrong_sdk_pin_fails_before_extraction(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = Path(temporary) / "sdk.zip", Path(temporary) / "system.ext4"
            source.write_bytes(b"not the pinned image")
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                extract_system(source, target)
            self.assertFalse(target.exists())

    def test_capex_exports_only_embedded_payload(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = Path(temporary) / "art.capex", Path(temporary) / "art.ext4"
            original = zip_bytes({"apex_payload.img": b"ext4 payload", "../../escape": b"bad"})
            source.write_bytes(zip_bytes({"original_apex": original, "../../escape": b"bad"}))
            self.assertEqual(unpack_apex(source, target), hashlib.sha256(original).hexdigest())
            self.assertEqual(target.read_bytes(), b"ext4 payload")
            self.assertEqual({p.name for p in Path(temporary).iterdir()}, {"art.capex", "art.apex", "art.ext4"})
            with self.assertRaises(FileExistsError):
                unpack_apex(source, target)

    def test_truncated_extent_and_existing_output_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "extent"
            with self.assertRaisesRegex(ValueError, "truncated"):
                copy_exact(io.BytesIO(b"short"), target, 100)
            with self.assertRaises(FileExistsError):
                copy_exact(io.BytesIO(b"new"), target, 3)
            self.assertEqual(target.read_bytes(), b"short")

    def test_debugfs_silent_dump_failure_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            image = Ext4("unused.ext4")
            with patch.object(image, "command", return_value=""):
                with self.assertRaisesRegex(ValueError, "missing or incomplete"):
                    image.dump("/missing", Path(temporary) / "missing")
                with self.assertRaisesRegex(ValueError, "missing or empty"):
                    image.entries("/missing")

    def test_tree_ignores_image_symlinks_and_checks_sizes(self):
        with tempfile.TemporaryDirectory() as temporary:
            image = Ext4("unused.ext4")
            listing = "/2/040755/0/0/.//\n/3/120777/0/0/libc.so/29/\n\n"
            with patch.object(image, "command", return_value=listing):
                image.tree("/lib64", Path(temporary))
            self.assertEqual(list(Path(temporary).iterdir()), [])
            target = Path(temporary) / "wrong-size"
            def dump(_):
                target.write_bytes(b"x")
                return ""
            with patch.object(image, "command", side_effect=dump):
                with self.assertRaisesRegex(ValueError, "incomplete"):
                    image.dump("/file", target, 2)

    def test_debugfs_quotes_paths_and_rejects_newline(self):
        self.assertEqual(quote_debugfs('a b"c\\d'), '"a b\\"c\\\\d"')
        with self.assertRaises(ValueError):
            quote_debugfs("a\nb")

    def test_wrong_architecture_elf_rejected_before_readelf(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "x86.so"
            header = bytearray(64)
            header[:6] = b"\x7fELF\x02\x01"
            struct.pack_into("<H", header, 18, 62)
            path.write_bytes(header)
            with self.assertRaisesRegex(ValueError, "ARM64"):
                elf_needed(path)

    def test_dex_checksum_detects_corruption(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "classes.dex"
            data = bytearray(112)
            data[:8] = b"dex\n035\0"
            struct.pack_into("<III", data, 32, len(data), 112, 0x12345678)
            data[12:32] = hashlib.sha1(data[32:]).digest()
            struct.pack_into("<I", data, 8, zlib.adler32(data[12:]))
            path.write_bytes(data)
            self.assertEqual(validate_dex(path), hashlib.sha256(data).hexdigest())
            data[-1] ^= 1
            path.write_bytes(data)
            with self.assertRaisesRegex(ValueError, "checksum"):
                validate_dex(path)


class ArtEvidenceTest(unittest.TestCase):
    def setUp(self):
        self.records = ["SCARLET_ANDROID_ART_START", "SCARLET_ART_BEGIN", "Scarlet ART: 42",
                        "SCARLET_ART_OK", "SCARLET_ANDROID_ART_EXIT 0", "SCARLET_ANDROID_ART_OK"]

    def result(self, records):
        return analyze_art(("\r\n".join(records) + "\r\n").encode())["result"]

    def test_java_and_exit_required_in_order(self):
        self.assertEqual(self.result(self.records), "PASS")
        for i in range(len(self.records)):
            self.assertEqual(self.result(self.records[:i] + self.records[i + 1:]), "INCOMPLETE")
        self.assertEqual(self.result(self.records[::-1]), "INCOMPLETE")

    def test_crash_duplicate_or_failure_after_java_output_is_not_pass(self):
        records = list(self.records)
        records[-2] = "SCARLET_ANDROID_ART_EXIT 134"
        self.assertEqual(self.result(records), "INCOMPLETE")
        self.assertEqual(self.result(self.records + ["SCARLET_ART_OK"]), "INCOMPLETE")
        self.assertEqual(self.result(self.records + ["SCARLET_ANDROID_ART_FAIL: waitpid"]), "INCOMPLETE")


if __name__ == "__main__":
    unittest.main()
