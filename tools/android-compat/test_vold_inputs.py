import json
from pathlib import Path
import tempfile
import unittest

from art_inputs import sha256
import vold_inputs


class VoldInputsTest(unittest.TestCase):
    def fixture(self, base):
        inputs = base / "inputs"
        build = base / "build"
        inputs.mkdir()
        build.mkdir()
        binary = b"stock-independent vold helper"
        (build / "vold_prepare_subdirs").write_bytes(binary)
        record = {
            "kind": vold_inputs.BUILD_KIND,
            "result": "BUILT",
            "source_sha256": sha256(vold_inputs.C_SOURCE),
            "binary_sha256": sha256(build / "vold_prepare_subdirs"),
            "label_policy": "host-managed",
        }
        (build / "build.json").write_text(json.dumps(record))
        profile = {
            "host_services": ["vold"],
            "subjects": {"u:r:vold:s0": 1, "u:r:vold_prepare_subdirs:s0": 2},
            "exec_transitions": [["u:r:vold:s0",
                                  "u:object_r:vold_prepare_subdirs_exec:s0",
                                  "u:r:vold_prepare_subdirs:s0", 0]],
        }
        return inputs, build, {"profile": profile}

    def test_install_binds_the_built_helper_to_vold(self):
        with tempfile.TemporaryDirectory() as temporary:
            inputs, build, metadata = self.fixture(Path(temporary))
            vold_inputs.install(inputs, metadata, build)
            vold_inputs.validate_staged(metadata, inputs)
            self.assertTrue((inputs / vold_inputs.BINARY).stat().st_mode & 0o111)

    def test_install_rejects_changed_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            inputs, build, metadata = self.fixture(Path(temporary))
            (build / "vold_prepare_subdirs").write_bytes(b"replaced")
            with self.assertRaises(ValueError):
                vold_inputs.install(inputs, metadata, build)


if __name__ == "__main__":
    unittest.main()
