"""Small host format checks; no downloaded images, Docker or VM needed."""
import importlib.util
import io
from pathlib import Path
import struct
import tempfile
import unittest
import zlib

spec = importlib.util.spec_from_file_location("cuttlefish", Path(__file__).resolve().parents[1] / "tools/prepare-cuttlefish.py")
cf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cf)


class ImageFormats(unittest.TestCase):
    def test_panic_classification(self):
        panic = cf.android.smoke.PANIC
        for line in [b"[Scarlet Kernel] panic: cpu=0 panicked at kernel/src/lib.rs:1",
                     b"[ 18.53][ T1] Kernel panic - not syncing: Attempted to kill init!",
                     b"thread 'main' panicked at src/main.rs:12:\n",
                     b"[panic] native init failed"]:
            self.assertIsNotNone(panic.search(b"\n" + line), line)
        for line in [b"09-28 18:24:00.887 611 611 E android.hardware.uwb: uwb_default_hal: panicked at hardware/interfaces/uwb/aidl/default/src/uwb_chip.rs:54:14:",
                     b"09-28 18:24:05.866 618 618 E bluetooth-cf: main: panicked at device/google/cuttlefish/guest/hals/bluetooth/src/hci.rs:114:14:"]:
            self.assertIsNone(panic.search(line), line)
        # A truncated rolling buffer is not a new line/start of the stream.
        # This used to invent a panic when the timestamp fell off the buffer.
        tail = b"09-28 18:24:00 E HAL: panicked at" + b"x" * (512 - len(b"panicked at"))
        tail = tail[-512:]
        self.assertTrue(tail.startswith(b"panicked at"))
        self.assertIsNone(panic.search(tail + b"\n"))

    def test_sparse_mixed_chunks(self):
        # RAW, nonzero FILL, DONT_CARE, zero FILL, CRC. End in a hole to exercise
        # truncation as well as readback, and use extended header lengths.
        chunks = [(0xCAC1, 1, b"R" * 4096), (0xCAC2, 2, b"ABCD"),
                  (0xCAC3, 3, b""), (0xCAC2, 1, bytes(4)), (0xCAC4, 0, bytes(4))]
        data = struct.pack("<I4H4I", 0xED26FF3A, 1, 0, 32, 16, 4096, 7, len(chunks), 0) + bytes(4)
        for kind, count, payload in chunks:
            data += struct.pack("<2H2I", kind, 0, count, 16 + len(payload)) + bytes(4) + payload
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "raw.img"
            cf.unsparse(io.BytesIO(data), target)
            self.assertEqual(target.read_bytes(), b"R" * 4096 + b"ABCD" * 2048 + bytes(4 * 4096))
            with self.assertRaises(ValueError):
                cf.unsparse(io.BytesIO(data[:-1]), Path(directory) / "truncated.img")
            with self.assertRaises(ValueError):
                cf.unsparse(io.BytesIO(data + b"unexpected"), Path(directory) / "extra.img")

    def test_gpt_backup_partition_data_and_unicode(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            payloads = [b"A" * 512 + bytes(1536), bytes(512) + b"Z" * 512]
            inputs = []
            for i, payload in enumerate(payloads):
                source = directory / f"{i}.img"
                source.write_bytes(payload)
                inputs.append((['super', 'metadata'][i], source))
            image = directory / "disk.img"
            cf.gpt_image(image, inputs)
            data = image.read_bytes()
            self.assertEqual(data[510:512], b"\x55\xaa")
            headers = []
            for offset in [512, len(data) - 512]:
                header = bytearray(data[offset:offset + 92])
                self.assertEqual(header[:8], b"EFI PART")
                expected = struct.unpack_from("<I", header, 16)[0]
                struct.pack_into("<I", header, 16, 0)
                self.assertEqual(zlib.crc32(header), expected)
                table_lba, count, size, crc = struct.unpack_from("<QIII", header, 72)
                table = data[table_lba * 512:table_lba * 512 + count * size]
                self.assertEqual(zlib.crc32(table), crc)
                headers.append((header, table))
            self.assertEqual(headers[0][1], headers[1][1])
            self.assertEqual(struct.unpack_from('<QQ', headers[0][0], 24), (1, len(data) // 512 - 1))
            self.assertEqual(struct.unpack_from('<QQ', headers[1][0], 24), (len(data) // 512 - 1, 1))
            for i, (name, _) in enumerate(inputs):
                record = headers[0][1][i * 128:(i + 1) * 128]
                self.assertEqual(record[56:].decode('utf-16le').rstrip('\0'), name)
                first, last = struct.unpack_from('<QQ', record, 32)
                self.assertEqual(first % 2048, 0)
                self.assertEqual(data[first * 512:(last + 1) * 512], payloads[i])
            # Never replace an existing disk, even if the input is valid.
            with self.assertRaises(FileExistsError):
                cf.gpt_image(image, inputs)


if __name__ == '__main__':
    unittest.main()
