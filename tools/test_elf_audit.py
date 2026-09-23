"""Program-header-only fixtures, including malformed inputs and loader blockers."""
from pathlib import Path
import struct
import tempfile
import unittest

from elf_audit import Elf, ElfError


def fixture(tags=(), relocations=(), machine=243, tls=False):
    data = bytearray(2048)
    data[:16] = b"\x7fELF\x02\x01\x01\x53" + bytes(8)
    phnum = 3 if tls else 2
    struct.pack_into("<HHIQQQIHHHHHH", data, 16, 3, machine, 1, 0, 64, 0, 0, 64, 56, phnum, 0, 0, 0)
    dynamic = [(5, 1024), (10, 64), (6, 1536), (11, 24)] + list(tags)
    if relocations:
        dynamic += [(7, 1280), (8, 24 * len(relocations)), (9, 24)]
        for i, (kind, sym) in enumerate(relocations):
            struct.pack_into("<QQq", data, 1280 + i * 24, 1700 + i * 8, (sym << 32) | kind, 0)
    dynamic.append((0, 0))
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 6, 0, 0, 0, len(data), len(data), 4096)
    struct.pack_into("<IIQQQQQQ", data, 120, 2, 6, 512, 512, 0, len(dynamic) * 16, len(dynamic) * 16, 8)
    if tls:
        struct.pack_into("<IIQQQQQQ", data, 176, 7, 4, 1800, 1800, 0, 4, 16, 16)
    for i, entry in enumerate(dynamic):
        struct.pack_into("<qQ", data, 512 + i * 16, *entry)
    strings = b"\0libdriver.so\0scarlet_symbol\0"
    data[1024:1024 + len(strings)] = strings
    struct.pack_into("<IBBHQQ", data, 1536 + 24, 14, 0x12, 0, 0, 0, 0)
    return data


class AuditTests(unittest.TestCase):
    def parse(self, data):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "fixture.elf"
            path.write_bytes(data)
            return Elf(path).report()

    def test_stripped_sectionless_dso(self):
        report = self.parse(fixture(tags=[(1, 1)], relocations=[(3, 0), (5, 1)]))
        self.assertEqual(report["needed"], ["libdriver.so"])
        self.assertEqual(report["relocations"], {"JUMP_SLOT": 1, "RELATIVE": 1})
        self.assertEqual(report["undefined_relocated_symbols"][0]["name"], "scarlet_symbol")
        self.assertEqual(report["osabi"], 0x53)

    def test_aarch64_features_are_exposed(self):
        report = self.parse(fixture(tags=[(30, 4), (36, 1900), (0x6ffffff0, 1920)],
                                    relocations=[(1031, 0), (1032, 0)], machine=183, tls=True))
        self.assertEqual(report["machine"], "aarch64")
        self.assertEqual(report["tls"], [{"filesz": 4, "memsz": 16, "align": 16}])
        self.assertTrue(report["textrel"] and report["relr"] and report["symbol_versioning"])
        self.assertEqual(report["relocations"], {"IRELATIVE": 1, "TLSDESC": 1})

    def test_truncated_program_headers(self):
        with self.assertRaises(ElfError):
            self.parse(fixture()[:80])

    def test_bad_virtual_address(self):
        data = fixture(tags=[(1, 1)])
        struct.pack_into("<Q", data, 520, 999999)
        with self.assertRaisesRegex(ElfError, "not file-backed"):
            self.parse(data)

    def test_string_beyond_table(self):
        with self.assertRaisesRegex(ElfError, "string offset"):
            self.parse(fixture(tags=[(1, 64)]))

    def test_duplicate_scalar_tag(self):
        with self.assertRaisesRegex(ElfError, "duplicate"):
            self.parse(fixture(tags=[(1, 1), (10, 2)]))

    def test_partial_relocation(self):
        with self.assertRaisesRegex(ElfError, "relocation entry size"):
            self.parse(fixture(tags=[(7, 1280), (8, 1), (9, 24)]))

    def test_foreign_format(self):
        with self.assertRaisesRegex(ElfError, "ELF64"):
            self.parse(b"\xcf\xfa\xed\xfe" + bytes(100))

    def test_riscv_tls_descriptor_number(self):
        report = self.parse(fixture(relocations=[(12, 0)]))
        self.assertEqual(report["relocations"], {"TLSDESC": 1})


if __name__ == "__main__":
    unittest.main()
