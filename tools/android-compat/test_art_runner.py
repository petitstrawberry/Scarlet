"""Host orchestration tests: subprocesses are simulated, never runtime evidence."""
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zlib

from art_inputs import SDK_SHA256, art_launch, sha256, verify_art_runtime

spec = importlib.util.spec_from_file_location("direct_runner", Path(__file__).with_name("run-direct.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


def runtime_fixture(root):
    # These dummy files test identity/staging only. Real ELF extraction is
    # independently validated against the pinned SDK by prepare-art.py.
    names = ["apex/com.android.art/bin/dalvikvm64", "apex/com.android.art/lib64/libart.so",
             "apex/com.android.art/javalib/core-oj.jar", "apex/com.android.runtime/bin/linker64",
             "system/etc/public.libraries.txt"]
    names += ["apex/com.android.runtime/lib64/bionic/" + name for name in ("libc.so", "libm.so", "libdl.so")]
    for name in names:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(name.encode())
    for directory in ("system/bin", "system/lib64", "data/local/tmp"):
        (root / directory).mkdir(parents=True, exist_ok=True)
    (root / "system/bin/linker64").symlink_to("../../apex/com.android.runtime/bin/linker64")
    metadata = {"result": "PREPARED", "sdk": {"sha256": SDK_SHA256},
                "files": {name: sha256(root / name) for name in names},
                "library_path": "/system/lib64", "boot_class_path": "/apex/com.android.art/javalib/core-oj.jar"}
    (root / "art-runtime.json").write_text(json.dumps(metadata))
    return metadata


class ArtRunnerTest(unittest.TestCase):
    def test_manifest_detects_changes_extras_and_escaping_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = runtime_fixture(root)
            self.assertEqual(verify_art_runtime(root), metadata)
            extra = root / "system/lib64/injected.so"
            extra.write_bytes(b"unrecorded")
            with self.assertRaisesRegex(ValueError, "unrecorded"):
                verify_art_runtime(root)
            extra.unlink()
            lib = root / "apex/com.android.art/lib64/libart.so"
            lib.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                verify_art_runtime(root)
            lib.unlink()
            lib.symlink_to("/etc/passwd")
            with self.assertRaisesRegex(ValueError, "invalid ART manifest path"):
                verify_art_runtime(root)

    def test_art_mode_stages_without_bionic_and_rejects_false_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / "runtime"
            root.mkdir()
            metadata = runtime_fixture(root)
            kernel = base / "scarlet"
            kernel.write_bytes(b"test kernel identity")
            firmware = base / "firmware.fd"
            firmware.write_bytes(b"firmware")
            dex = base / "classes.dex"
            data = bytearray(112)
            data[:8] = b"dex\n035\0"
            struct.pack_into("<III", data, 32, 112, 112, 0x12345678)
            data[12:32] = hashlib.sha1(data[32:]).digest()
            struct.pack_into("<I", data, 8, zlib.adler32(data[12:]))
            dex.write_bytes(data)
            expected = art_launch(metadata)
            for complete in (False, True):
                output = base / str(complete)
                calls = []
                def build(command, **kwargs):
                    calls.append(command)
                    if command[0] == "cargo":
                        self.assertEqual(kwargs["env"]["SCARLET_ART_ARGS"].split("\x1f"), expected["argv"])
                        self.assertEqual(kwargs["env"]["SCARLET_ART_ENV"].split("\x1f"), expected["env"])
                        self.assertEqual(kwargs["env"]["SCARLET_ANDROID_ART"], "1")
                        directory = output / "target/aarch64-unknown-scarlet/release"
                        directory.mkdir(parents=True, exist_ok=True)
                        for name in ("android-direct-init", "scarlet-ld"):
                            (directory / name).write_bytes(b"simulated compiler output")
                def guest(command, destination, timeout):
                    self.assertIn("qemu-system-aarch64", command)
                    self.assertEqual(timeout, 120)
                    lines = ["SCARLET_ANDROID_ART_START"]
                    if complete:
                        lines += ["SCARLET_ART_BEGIN", "Scarlet ART: 42", "SCARLET_ART_OK"]
                    lines += ["SCARLET_ANDROID_ART_EXIT 0", "SCARLET_ANDROID_ART_OK"]
                    (destination / "serial.log").write_text("\n".join(lines) + "\n")
                    return {"result": "PASS"}
                args = ["run-direct.py", "--kernel", str(kernel), "--runtime-root", str(root),
                        "--art-dex", str(dex), "--output", str(output)]
                with patch("sys.argv", args), patch.object(runner.smoke, "firmware", return_value=firmware), \
                        patch.object(runner.subprocess, "run", side_effect=build), \
                        patch.object(runner.smoke, "run_guest", side_effect=guest):
                    self.assertEqual(runner.main(), 0 if complete else 1)
                self.assertEqual(json.loads((output / "art-launch.json").read_text()), expected)
                self.assertEqual((output / "staging/systems/android-aarch64/data/local/tmp/classes.dex").read_bytes(), data)
                self.assertFalse((output / "probes.json").exists())
                self.assertEqual(json.loads((output / "inputs.json").read_text())["dex_sha256"], sha256(dex))
                self.assertEqual([c[0] for c in calls], ["cargo", "cargo", "mkfs.ext2", "cargo-scarlet-plugin-limine"])


if __name__ == "__main__":
    unittest.main()
