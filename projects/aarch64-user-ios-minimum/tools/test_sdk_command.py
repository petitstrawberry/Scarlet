"""Exercise the installed cargo-scarlet's image/hook/runner wiring in temporary projects.

Only BSP compilation/metadata is stubbed; cargo-scarlet itself, filesystem image
composition, hook invocation and runner invocation are real. No app is repacked.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from sdk_command import commands


TOOLS = Path(__file__).resolve().parent


class SdkWiringTests(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory()
        self.addCleanup(self.work.cleanup)
        self.root = Path(self.work.name).resolve()
        self.project = self.root / "project"
        self.project.mkdir()
        config = self.project / "bsp/.cargo"
        config.mkdir(parents=True)
        (config / "config.toml").write_text('[build]\ntarget = "aarch64-apple-ios-sim"\n')
        (self.project / "payload").write_text("filesystem fixture")
        (self.project / "scarlet.toml").write_text('''schema_version = 2
[project]
name = "ios-packaging-test"
[bsp]
path = "bsp"
package = "scarlet"
[bsp.kernel]
source = { path = "kernel" }
[images.initramfs]
format = "newc"
output = ".scarlet/images/test.cpio"
[[images.initramfs.layers]]
kind = "copy"
source = "payload"
to = "/payload"
[hooks.post-image]
command = "package-test"
[runner]
command = "run-test"
''')
        for phase in ("package", "run"):
            script = self.project / f"{phase}-test"
            script.write_text(f"#!{sys.executable}\n" + f"phase = {phase!r}\n" + """
import json, os, sys
from pathlib import Path
sys.path.insert(0, os.environ['TEST_TOOLS'])
from image_config import image_path
executable = image_path(Path.cwd(), 'executable', 'aarch64-apple-ios-sim', 'release')
with open('events.jsonl', 'a') as events:
    events.write(json.dumps({'phase': phase, 'args': sys.argv[1:],
        'executable': str(executable), 'content': executable.read_text()}) + '\\n')
""")
            script.chmod(0o755)
        self.target = self.root / "custom target"
        self.executable = self.target / "aarch64-apple-ios-sim/release/scarlet"
        self.executable.parent.mkdir(parents=True)
        self.executable.write_text("prebuilt fixture")
        self.env = dict(os.environ, CARGO_TARGET_DIR="custom target", TEST_TOOLS=str(TOOLS))
        self.real_cargo = shutil.which("cargo")
        if not self.real_cargo or not shutil.which("cargo-scarlet"):
            self.skipTest("Run inside the Scarlet development shell with cargo-scarlet")

    def invoke(self, command, env=None):
        return subprocess.run(command, cwd=self.root, env=env or self.env,
                              text=True, capture_output=True)

    def events(self):
        return [json.loads(line) for line in (self.project / "events.jsonl").read_text().splitlines()]

    def test_real_sdk_reproduces_missing_canonical_binary_then_adapter_reaches_hook(self):
        args = ["image", "--project", str(self.project), "--release", "--locked", "--no-build"]
        before = self.invoke([self.real_cargo, "scarlet", *args])
        self.assertNotEqual(before.returncode, 0)
        self.assertIn("BSP executable not found", before.stderr)
        self.assertIn(str(self.project / "bsp/target/aarch64-apple-ios-sim/release/scarlet"), before.stderr)
        after = self.invoke([sys.executable, TOOLS / "sdk_command.py", str(self.project),
                             "image", "--release", "--locked", "--no-build"])
        self.assertEqual(after.returncode, 0, after.stderr)
        self.assertTrue((self.project / ".scarlet/images/test.cpio").is_file())
        self.assertEqual(self.events(), [{"phase": "package", "args": [],
            "executable": str(self.executable), "content": "prebuilt fixture"}])
        self.assertFalse((self.project / "bsp/target").exists())

    def test_custom_run_uses_real_sdk_image_then_real_runner(self):
        mock_bin = self.root / "mock-bin"
        mock_bin.mkdir()
        cargo = mock_bin / "cargo"
        cargo.write_text(f"#!{sys.executable}\n" + f"real_cargo = {self.real_cargo!r}\n" + """
import json, os, sys
from pathlib import Path
if sys.argv[1] == 'scarlet':
    os.execv(real_cargo, [real_cargo, *sys.argv[1:]])
elif sys.argv[1] == 'metadata':
    print(json.dumps({'packages': [], 'resolve': None}))
elif sys.argv[1] == 'build':
    target = sys.argv[sys.argv.index('--target') + 1]
    profile = 'release' if '--release' in sys.argv else 'debug'
    binary = Path(os.environ['CARGO_TARGET_DIR']) / target / profile / 'scarlet'
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_text('fresh BSP fixture')
else:
    raise SystemExit('Unexpected compiler-side Cargo call: ' + repr(sys.argv))
""")
        cargo.chmod(0o755)
        env = dict(self.env, PATH=str(mock_bin) + os.pathsep + self.env["PATH"])
        result = self.invoke([sys.executable, TOOLS / "sdk_command.py", str(self.project),
                              "run", "--release", "--locked", "--", "--display", "argument with spaces"], env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([e["phase"] for e in self.events()], ["package", "run"])
        self.assertEqual(self.events()[1]["args"], ["--display", "argument with spaces"])
        self.assertTrue(all(e["content"] == "fresh BSP fixture" for e in self.events()))
        self.assertFalse((self.project / "bsp/target").exists())

    def test_default_run_and_no_image_keep_original_sdk_commands(self):
        for target, args in ((self.project / "bsp/target", ["--release"]),
                             (self.target, ["--release", "--no-image", "--", "--display"])):
            with self.subTest(target=target, args=args), patch.dict(os.environ, CARGO_TARGET_DIR=str(target)):
                self.assertEqual(commands(self.project, "run", args),
                    [["cargo", "scarlet", "run", "--project", str(self.project), *args]])

    def test_image_override_tracks_explicit_target_and_profile(self):
        for args, profile in ((["--target=aarch64-apple-ios", "--no-build"], "debug"),
                              (["--target", "aarch64-apple-ios", "--release"], "release")):
            with self.subTest(args=args), patch.dict(os.environ, CARGO_TARGET_DIR=str(self.target)):
                plan = commands(self.project, "image", args)
                self.assertEqual(plan, [["cargo", "scarlet", "image", "--project", str(self.project),
                    *args, "--kernel-elf", str(self.target / "aarch64-apple-ios" / profile / "scarlet")]])


if __name__ == "__main__":
    unittest.main()
