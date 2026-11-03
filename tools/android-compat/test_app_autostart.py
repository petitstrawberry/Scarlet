"""Exercise the manager's launch lifecycle with controlled stock-cmd results."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class AppAutostartTest(unittest.TestCase):
    def run_launch(self, unlocked=True, install_status=0):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            command = root / "cmd"
            command.write_text("""#!/bin/sh
echo "$*" >> "$CALLS"
if [ "$1 $2" = 'activity get-started-user-state' ]; then
    n=0; [ ! -f "$COUNT" ] || n=$(cat "$COUNT")
    n=$((n + 1)); echo "$n" > "$COUNT"
    if [ "$UNLOCKED" = 1 ] && [ "$n" -ge 3 ]; then echo RUNNING_UNLOCKED
    else echo RUNNING_UNLOCKING; fi
elif [ "$1 $2" = 'package install-existing' ]; then exit "$INSTALL_STATUS"
elif [ "$1 $2" = 'activity start' ]; then echo 'Status: ok'
else exit 90; fi
""")
            command.chmod(0o755)
            sleep = root / "toybox"
            sleep.write_text("#!/bin/sh\nexit 0\n")
            sleep.chmod(0o755)
            script = root / "launch.sh"
            script.write_text(Path(__file__).with_name("app-autostart.sh").read_text()
                              .replace("/system/bin/cmd", str(command))
                              .replace("/system/bin/toybox", str(sleep)))
            env = dict(os.environ, CALLS=str(root / "calls"), COUNT=str(root / "count"),
                       UNLOCKED=str(int(unlocked)), INSTALL_STATUS=str(install_status))
            result = subprocess.run(["/bin/sh", str(script), "com.example/com.example.Main"],
                                    env=env, capture_output=True, text=True, timeout=10)
            return result, (root / "calls").read_text().splitlines()

    def test_wait_then_launch_exactly_once_without_forcing_compilation(self):
        result, calls = self.run_launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["activity get-started-user-state 0"] * 3 + [
            "package install-existing --user 0 com.example",
            "activity start -W --user 0 -f 0x30000000 -n com.example/com.example.Main"])

    def test_locked_user_times_out_without_attempting_launch(self):
        result, calls = self.run_launch(unlocked=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, ["activity get-started-user-state 0"] * 120)

    def test_install_failure_does_not_trigger_launch(self):
        result, calls = self.run_launch(install_status=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(call.startswith("activity start") for call in calls))
