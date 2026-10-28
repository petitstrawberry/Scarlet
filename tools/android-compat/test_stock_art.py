from pathlib import Path
import json
import tempfile
import unittest
import xml.etree.ElementTree as ET

from art_inputs import SDK_SHA256, sha256
from stock_art import BIONIC_ALIASES, activation_xml, apex_identity, bounded_alias, decode_logd, linkerconfig_succeeded, merge_properties, verify_stock_inputs
from test_art_runner import runtime_fixture


class StockArtTest(unittest.TestCase):
    def test_original_apex_identity_and_activation(self):
        data = b"\x0a\x0fcom.android.art\x10\x7b\x2a\x03one\x3a\x05a.so!"
        self.assertEqual(apex_identity(data), ("com.android.art", 123, "one"))
        records = [dict(name="com.android.art", version=123, version_name="one",
                        source="/system/apex/com.android.art.capex")]
        row = ET.fromstring(activation_xml(records))[0].attrib
        self.assertEqual(row["modulePath"], "/data/apex/decompressed/com.android.art@123.decompressed.apex")
        self.assertEqual(row["preinstalledModulePath"], records[0]["source"])
        self.assertEqual(row["isActive"], "true")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            activation_xml(records + records)

    def test_malformed_or_ambiguous_protobuf_is_rejected(self):
        valid = b"\x0a\x0fcom.android.art\x10\x01"
        cases = [b"\x0a\x80", b"\x0a\x20short", b"\x0f", b"\x00\x01", b"\x80" * 10,
                 valid + b"\x10\x02", valid + b"\x2a\x01x\x2a\x01y",
                 b"\x0a\x06../bad\x10\x01", b"\x08\x01\x10\x01"]
        for data in cases:
            with self.subTest(data=data), self.assertRaises(ValueError):
                apex_identity(data)

    def test_partition_precedence_and_unhandled_syntax(self):
        with tempfile.TemporaryDirectory() as directory:
            system, vendor = Path(directory) / "system.prop", Path(directory) / "vendor.prop"
            system.write_text("# comment\nro.test=system\nro.empty=\n")
            vendor.write_text("ro.test = vendor\nro.vndk.version=33\n")
            self.assertEqual(merge_properties([system, vendor]), "ro.empty=\nro.test=vendor\nro.vndk.version=33\n")
            for line in ["import /other.prop", "ro.test=${other}", "not a name=value"]:
                vendor.write_text(line)
                with self.assertRaises(ValueError):
                    merge_properties([vendor])

    def test_library_aliases_preserve_guest_destination_inside_staged_root(self):
        self.assertEqual(bounded_alias("apex/com.android.i18n/lib64/libc++.so", "/system/lib64/libc++.so"),
                         "../../../system/lib64/libc++.so")
        self.assertEqual(bounded_alias("system/lib64/libc.so", "/apex/com.android.runtime/lib64/bionic/libc.so"),
                         "../../apex/com.android.runtime/lib64/bionic/libc.so")
        for target in ["/etc/passwd", "../../../../../../etc/passwd", "/system/lib64/a\n.so"]:
            with self.assertRaises(ValueError):
                bounded_alias("apex/com.android.i18n/lib64/file", target)

    def test_zero_exit_with_linkerconfig_error_is_not_success(self):
        packet = bytes(11) + b"\x06linkerconfig\x00not found: libdexfiled.so\x00"
        message = decode_logd(packet)
        self.assertEqual(message["priority"], 6)
        self.assertEqual(message["tag"], "linkerconfig")
        self.assertFalse(linkerconfig_succeeded(0, [message]))
        self.assertFalse(linkerconfig_succeeded(None, []))
        self.assertTrue(linkerconfig_succeeded(0, []))
        for data in [b"short", bytes(11) + b"\x06missing-terminator", bytes(16)]:
            with self.assertRaises(ValueError):
                decode_logd(data)

    def test_stock_overlay_detects_changed_files_aliases_and_base(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, stock = Path(directory) / "runtime", Path(directory) / "stock"
            runtime.mkdir()
            runtime_fixture(runtime)
            (stock / "system/lib64").mkdir(parents=True)
            data = stock / "system/build.prop"
            data.write_text("ro.build.version.sdk=33\n")
            for name, target in BIONIC_ALIASES.items():
                (stock / name).symlink_to(target)
            metadata = {"result": "PREPARED", "sdk": {"sha256": SDK_SHA256},
                        "base_manifest_sha256": sha256(runtime / "art-runtime.json"),
                        "files": {"system/build.prop": sha256(data)},
                        "directories": ["system", "system/lib64"], "symlinks": BIONIC_ALIASES}
            (stock / "stock-inputs.json").write_text(json.dumps(metadata))
            self.assertEqual(verify_stock_inputs(stock, runtime), metadata)
            data.write_text("changed")
            with self.assertRaisesRegex(ValueError, "changed"):
                verify_stock_inputs(stock, runtime)
            data.write_text("ro.build.version.sdk=33\n")
            alias = stock / "system/lib64/libc.so"
            alias.unlink()
            alias.symlink_to("/etc/passwd")
            with self.assertRaisesRegex(ValueError, "symlinks"):
                verify_stock_inputs(stock, runtime)
            alias.unlink()
            alias.symlink_to(BIONIC_ALIASES["system/lib64/libc.so"])
            with (runtime / "art-runtime.json").open("a") as stream:
                stream.write("\n")
            with self.assertRaisesRegex(ValueError, "mismatched"):
                verify_stock_inputs(stock, runtime)


if __name__ == "__main__":
    unittest.main()
