import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("fixture", Path(__file__).resolve().parents[1] / "tools/fixture.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class SerialInputTest(unittest.TestCase):
    def test_fifo_delivers_input_and_retains_serial_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            fifo = output / "input"
            os.mkfifo(fifo)
            fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
            try:
                os.write(fd, b"probe\n")
                command = [sys.executable, "-c", "import sys; assert sys.stdin.readline() == 'probe\\n'; print('SCARLET_CROSVM_GUEST_OK', flush=True)"]
                result = fixture.run_guest(command, output, 5, input_fifo=fifo)
                self.assertEqual(result["result"], "PASS")
                self.assertIn(b"SCARLET_CROSVM_GUEST_OK", (output / "serial.log").read_bytes())
            finally:
                os.close(fd)

    def test_regular_input_file_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            source = output / "input"
            source.write_text("not a FIFO")
            with self.assertRaises(ValueError):
                fixture.run_guest([], output, 5, input_fifo=source)


if __name__ == "__main__":
    unittest.main()
