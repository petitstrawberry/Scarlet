"""Packaging path regressions; no Cargo builds, Xcode calls or app repacking."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


TOOLS = Path(__file__).resolve().parent
MINIMUM = TOOLS.parent
DESKTOP = MINIMUM.parent / "aarch64-user-ios-desktop"


class ExecutableLookupTests(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory()
        self.addCleanup(self.work.cleanup)
        self.root = Path(self.work.name).resolve()
        self.caller = self.root / "caller directory"
        self.caller.mkdir()
        self.env = os.environ.copy()
        self.env.pop("CARGO_TARGET_DIR", None)

    def lookup(self, project, field, *args, cwd=None, env=None):
        return Path(subprocess.check_output([
            sys.executable, TOOLS / "image_config.py", project, field, *args,
        ], cwd=cwd or self.caller, env=env or self.env, text=True).strip())

    def test_both_flavors_default_to_shared_bsp_target(self):
        for project in (MINIMUM, DESKTOP):
            for value in (None, ""):
                with self.subTest(project=project.name, value=value):
                    if value is None:
                        self.env.pop("CARGO_TARGET_DIR", None)
                    else:
                        self.env["CARGO_TARGET_DIR"] = value
                    self.assertEqual(self.lookup(project, "target-dir"), MINIMUM / "bsp/target")

    def test_explicit_override_selects_target_and_profile(self):
        directory = self.root / "custom target"
        self.env["CARGO_TARGET_DIR"] = str(directory)
        for target in ("aarch64-apple-ios-sim", "aarch64-apple-ios"):
            for profile in ("debug", "release"):
                with self.subTest(target=target, profile=profile):
                    expected = directory / target / profile / "scarlet"
                    expected.parent.mkdir(parents=True)
                    expected.write_text("override executable")
                    actual = self.lookup(MINIMUM, "executable", target, profile)
                    self.assertEqual(actual, expected)
                    self.assertEqual(actual.read_text(), "override executable")

    def test_direct_packaging_resolves_relative_override_from_caller(self):
        self.env["CARGO_TARGET_DIR"] = "cache/../build artifacts"
        self.assertEqual(
            self.lookup(DESKTOP, "executable", "aarch64-apple-ios-sim", "release"),
            self.caller / "build artifacts/aarch64-apple-ios-sim/release/scarlet",
        )

    def run_wrapper(self, override):
        project = self.root / "selected project"
        project.mkdir()
        (project / "scarlet.toml").write_text('[bsp]\npath = "bsp"\n')
        config = project / "bsp/.cargo"
        config.mkdir(parents=True)
        (config / "config.toml").write_text('[build]\ntarget = "aarch64-apple-ios-sim"\n')
        mock_bin = self.root / "mock-bin"
        mock_bin.mkdir()
        # Only sysroot preparation is stubbed. Path resolution uses the real tool.
        python = mock_bin / "python3"
        python.write_text(f"#!{sys.executable}\n" + """
import os, sys
if sys.argv[1].endswith('/prepare_guest_sysroot.py'):
    print('/unused-test-sysroot')
else:
    os.execv(sys.executable, [sys.executable, *sys.argv[1:]])
""")
        cargo = mock_bin / "cargo"
        cargo.write_text(f"#!{sys.executable}\n" + """
import json, os, subprocess, sys
from pathlib import Path
project = Path(sys.argv[sys.argv.index('--project') + 1])
os.chdir(project)  # The SDK invokes its post-image hook from the project.
executable = subprocess.check_output([
    sys.executable, os.environ['TEST_IMAGE_CONFIG'], str(project), 'executable',
    'aarch64-apple-ios-sim', 'release',
], text=True).strip()
print(json.dumps({'target_dir': os.environ['CARGO_TARGET_DIR'],
                  'executable': executable, 'cwd': str(Path.cwd())}))
""")
        for executable in (python, cargo):
            executable.chmod(0o755)
        env = dict(self.env, SCARLET_IOS_PROJECT=str(project),
                   SCARLET_IOS_DEVELOPER_DIR="/unused-test-xcode",
                   TEST_IMAGE_CONFIG=str(TOOLS / "image_config.py"),
                   PATH=str(mock_bin) + os.pathsep + self.env["PATH"])
        if override is not None:
            env["CARGO_TARGET_DIR"] = override
        result = json.loads(subprocess.check_output([
            TOOLS / "scarlet_ios.sh", "image", "--release", "--locked",
        ], cwd=self.caller, env=env, text=True))
        self.assertEqual(result["cwd"], str(project))
        return project, result

    def test_wrapper_keeps_absolute_override_through_sdk_cwd_change(self):
        expected = self.root / "custom target"
        _, result = self.run_wrapper(str(expected))
        self.assertEqual(result["target_dir"], str(expected))
        self.assertEqual(result["executable"], str(expected / "aarch64-apple-ios-sim/release/scarlet"))

    def test_wrapper_normalizes_relative_override_before_sdk_cwd_change(self):
        expected = self.caller / "build artifacts"
        _, result = self.run_wrapper("cache/../build artifacts")
        self.assertEqual(result["target_dir"], str(expected))
        self.assertEqual(result["executable"], str(expected / "aarch64-apple-ios-sim/release/scarlet"))

    def test_wrapper_default_uses_selected_bsp(self):
        project, result = self.run_wrapper(None)
        expected = project / "bsp/target"
        self.assertEqual(result["target_dir"], str(expected))
        self.assertEqual(result["executable"], str(expected / "aarch64-apple-ios-sim/release/scarlet"))


if __name__ == "__main__":
    unittest.main()
