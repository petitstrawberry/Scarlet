import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest
import zlib

import user_unlock_inputs
from art_inputs import sha256


def dex_bytes(size=112):
    data = bytearray(size)
    data[:8] = b"dex\n035\0"
    struct.pack_into("<III", data, 32, len(data), 112, 0x12345678)
    data[12:32] = hashlib.sha1(data[32:]).digest()
    struct.pack_into("<I", data, 8, zlib.adler32(data[12:]))
    return bytes(data)


class UserUnlockInputsTest(unittest.TestCase):
    def fixture(self, base):
        inputs = base / "inputs"
        build = base / "build"
        inputs.mkdir()
        build.mkdir()
        (inputs / "system/bin").mkdir(parents=True)
        (inputs / "scarlet/boot").mkdir(parents=True)
        (inputs / "system/bin/app_process64").write_bytes(b"stock app_process64")
        (inputs / "scarlet/boot/init.rc").write_text("on early-init\n")
        (build / "classes.dex").write_bytes(dex_bytes())
        record = {
            "kind": user_unlock_inputs.BUILD_KIND,
            "result": "BUILT",
            "main_class": user_unlock_inputs.JAVA_SOURCE.stem,
            "source_sha256": sha256(user_unlock_inputs.JAVA_SOURCE),
            "dex_sha256": sha256(build / "classes.dex"),
        }
        (build / "build.json").write_text(json.dumps(record))
        profile = {
            "subjects": {"u:r:init:s0": 1, "u:r:user_unlock:s0": 2},
            "exec_transitions": [["u:r:init:s0", "u:object_r:user_unlock_exec:s0",
                                  "u:r:user_unlock:s0", 1000]],
        }
        return inputs, build, {"profile": profile}

    def test_install_accepts_only_the_matching_build_and_stock_runner(self):
        with tempfile.TemporaryDirectory() as temporary:
            inputs, build, metadata = self.fixture(Path(temporary))
            user_unlock_inputs.install(inputs, metadata, build)
            self.assertEqual((inputs / user_unlock_inputs.RUNNER).read_bytes(),
                             b"stock app_process64")
            self.assertIn("start scarlet-user-unlock",
                          (inputs / "scarlet/boot/init.rc").read_text())
            user_unlock_inputs.validate_staged(metadata, inputs)

    def test_install_rejects_a_tampered_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            inputs, build, metadata = self.fixture(Path(temporary))
            record = json.loads((build / "build.json").read_text())
            record["source_sha256"] = "0" * 64
            (build / "build.json").write_text(json.dumps(record))
            with self.assertRaises(ValueError):
                user_unlock_inputs.install(inputs, metadata, build)


if __name__ == "__main__":
    unittest.main()
