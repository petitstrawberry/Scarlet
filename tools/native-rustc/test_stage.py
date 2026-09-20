"""Keep native startup objects in the staged compiler's link inputs."""
from contextlib import redirect_stderr, redirect_stdout
import io
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch

from audit_elf import ElfError
import stage


def object_bytes(machine=183):
    data = bytearray(200)
    data[:16] = b"\x7fELF\x02\x01\x01\0" + bytes(8)
    struct.pack_into("<HHIQQQIHHHHHH", data, 16, 1, machine, 1, 0, 0, 64, 0, 64, 0, 0, 64, 2, 0)
    struct.pack_into("<IIQQQQIIQQ", data, 128, 0, 1, 6, 0, 192, 8, 0, 0, 4, 0)
    return data


def executable_bytes(interpreter=None):
    data = bytearray(256)
    data[:16] = b"\x7fELF\x02\x01\x01\x53" + bytes(8)
    struct.pack_into("<HHIQQQIHHHHHH", data, 16, 2, 183, 1, 192, 64, 0, 0, 64, 56,
                     2 if interpreter else 1, 0, 0, 0)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0, 0, len(data), len(data), 4096)
    if interpreter:
        value = interpreter.encode() + b"\0"
        data[192:192 + len(value)] = value
        struct.pack_into("<IIQQQQQQ", data, 120, 3, 4, 192, 192, 0, len(value), len(value), 1)
    return data


class StageTests(unittest.TestCase):
    def prepare(self, root):
        source = root / "sysroot"
        target_lib = Path("lib/rustlib/aarch64-unknown-scarlet/lib")
        for name, content in {
            Path("bin/rustc"): executable_bytes(stage.INTERPRETER),
            Path("lib/librustc_driver-fixture.so"): executable_bytes(),
            target_lib / "libstd-fixture.rlib": b"archive",
            target_lib / "scarlet-crt0.o": object_bytes(),
            target_lib / "self-contained/libfixture.a": b"archive",
        }.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        loader = root / "scarlet-ld"
        loader.write_bytes(executable_bytes())
        probe = root / "probe"
        probe.write_bytes(executable_bytes())
        argv = ["stage.py", "--sysroot", str(source), "--target", "aarch64-unknown-scarlet",
                "--source-commit", "a" * 40, "--output", str(root / "overlay"),
                "--loader", str(loader), "--probe", str(probe)]
        return argv, source / target_lib

    def test_stages_startup_object_and_nested_archive(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            argv, libs = self.prepare(root)
            with patch.object(sys, "argv", argv), redirect_stdout(io.StringIO()):
                self.assertEqual(stage.main(), 0)
            staged = root / "overlay/opt/native-rustc/lib/rustlib/aarch64-unknown-scarlet/lib"
            self.assertEqual((staged / "scarlet-crt0.o").read_bytes(), (libs / "scarlet-crt0.o").read_bytes())
            self.assertTrue((staged / "self-contained/libfixture.a").is_file())
            manifest = json.loads((root / "overlay/native-rustc-manifest.json").read_text())
            report = manifest["elf"][str((staged / "scarlet-crt0.o").relative_to(root / "overlay"))]
            self.assertEqual(report["elf_type"], "REL")

    def test_rejects_foreign_startup_object_before_creating_overlay(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            argv, libs = self.prepare(root)
            (libs / "scarlet-crt0.o").write_bytes(object_bytes(machine=243))
            with patch.object(sys, "argv", argv), redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                stage.main()
            self.assertFalse((root / "overlay").exists())

    def test_rejects_truncated_object_sections(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "crt.o"
            for data in (object_bytes()[:100], object_bytes()[:-1]):
                path.write_bytes(data)
                with self.assertRaises(ElfError):
                    stage.object_report(path)


if __name__ == "__main__":
    unittest.main()
