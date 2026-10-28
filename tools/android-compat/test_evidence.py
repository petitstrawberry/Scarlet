import base64
import struct
import unittest
import zlib

from evidence import analyze, analyze_direct, validate_png


def png():
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0)) +
            chunk(b"IDAT", zlib.compress(b"\x00\x10\x20\x30\xff")) + chunk(b"IEND", b""))


class EvidenceTest(unittest.TestCase):
    def test_direct_exit_zero_without_child_output_is_not_success(self):
        lines = []
        for case in ["libc", "thread", "shared", "signal", "exec", "xattr", "mremap", "binder-absent"]:
            lines += [f"SCARLET_ANDROID_DIRECT_CASE /bin/bionic-static {case}",
                      f"SCARLET_ANDROID_DIRECT_EXIT /bin/bionic-static {case} 0"]
        lines += ["SCARLET_ANDROID_DIRECT_OK"]
        self.assertEqual(analyze_direct("\n".join(lines).encode())["result"], "INCOMPLETE")
        for case in ["libc", "thread", "shared", "signal", "exec", "xattr", "mremap", "binder-absent"]:
            at = lines.index(f"SCARLET_ANDROID_DIRECT_EXIT /bin/bionic-static {case} 0")
            lines[at:at] = [f"SCARLET_BIONIC_BEGIN {case}", f"SCARLET_BIONIC_OK {case}"]
        data = "\n".join(lines).encode()
        self.assertEqual(analyze_direct(data)["result"], "PASS")
        self.assertEqual(analyze_direct(data, dynamic=True)["result"], "INCOMPLETE")
        self.assertEqual(analyze_direct(data, binder=True)["result"], "INCOMPLETE")

    def test_boot_marker_does_not_certify_capture(self):
        result, frame = analyze(b"SCARLET_ANDROID_BOOT_OK\r\n")
        self.assertTrue(result["boot_completed"])
        self.assertEqual(result["result"], "INCOMPLETE")
        self.assertIsNone(frame)

    def test_capture_and_apk_require_distinct_milestones(self):
        data = (b"SCARLET_ANDROID_BOOT_OK\r\nSCARLET_APK_FRAME_BEGIN\r\n" + base64.b64encode(png()) +
                b"\r\nSCARLET_APK_FRAME_END\r\nSCARLET_ANDROID_APK_OK\r\n")
        result, frame = analyze(data, apk=True)
        self.assertIsNotNone(frame)
        self.assertEqual(result["result"], "INCOMPLETE")
        data += b"SCARLET_ANDROID_APK_RESUMED\nSCARLET_ANDROID_APK_INPUT_OK\n"
        self.assertEqual(analyze(data, apk=True)[0]["result"], "PASS")
        self.assertEqual(analyze(data + b"SCARLET_ANDROID_APK_FAIL: crash\n", apk=True)[0]["result"], "INCOMPLETE")

    def test_corrupt_or_truncated_capture_is_rejected(self):
        for data in [png()[:-1], png() + b"trailing", png()[:40] + b"bad" + png()[43:]]:
            with self.assertRaises(ValueError):
                validate_png(data)

    def test_log_interleaving_is_not_silently_discarded(self):
        data = b"SCARLET_FRAME_BEGIN\n" + base64.b64encode(png()) + b"\n[log] interrupted\nSCARLET_FRAME_END\n"
        self.assertIsNone(analyze(data)[1])


if __name__ == "__main__":
    unittest.main()
