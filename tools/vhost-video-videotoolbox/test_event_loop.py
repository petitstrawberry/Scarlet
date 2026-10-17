#!/usr/bin/env python3
"""Exercise the built daemon with macOS pipe-backed vhost-user notifications."""

import array
import mmap
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unittest


BINARY = Path(__file__).parent / ".build/release/vhost-video-videotoolbox"


class EventLoopTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="scarlet-video-test-")
        self.addCleanup(self.temp.cleanup)
        self.log = open(Path(self.temp.name) / "daemon.log", "w+")
        self.addCleanup(self.log.close)
        path = str(Path(self.temp.name) / "video.sock")
        self.process = subprocess.Popen(
            [str(BINARY), "--socket", path], stdout=self.log, stderr=self.log
        )
        self.addCleanup(self.stop)
        self.pipe_writers = set()
        self.addCleanup(self.close_pipes)
        self.conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(self.conn.close)
        self.conn.settimeout(2)
        deadline = time.monotonic() + 5
        while True:
            try:
                self.conn.connect(path)
                break
            except FileNotFoundError:
                if self.process.poll() is not None or time.monotonic() >= deadline:
                    self.fail("daemon did not start")
                time.sleep(0.01)

        self.memory_file = tempfile.TemporaryFile(dir=self.temp.name)
        self.addCleanup(self.memory_file.close)
        self.memory_file.truncate(262144)
        self.memory = mmap.mmap(self.memory_file.fileno(), 262144)
        self.addCleanup(self.memory.close)
        self.send(5, struct.pack("<IIQQQQ", 1, 0, 0x1000, 262144, 0x1000, 0),
                  self.memory_file.fileno())
        self.send(8, struct.pack("<II", 0, 8))
        self.send(9, struct.pack("<IIQQQQ", 0, 0, 0x1000, 0x1400, 0x1200, 0))
        self.send(18, struct.pack("<II", 0, 1))

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
        self.process.wait(timeout=5)

    def close_pipes(self):
        for fd in self.pipe_writers:
            os.close(fd)

    def send(self, request, payload=b"", fd=None):
        header = struct.pack("<III", request, 1, len(payload))
        ancillary = [] if fd is None else [
            (socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))
        ]
        self.assertEqual(self.conn.sendmsg([header], ancillary), len(header))
        self.conn.sendall(payload)

    def receive(self, count):
        data = b""
        while len(data) < count:
            chunk = self.conn.recv(count - len(data))
            self.assertTrue(chunk, "daemon disconnected")
            data += chunk
        return data

    def sync(self):
        self.send(17)
        self.assertEqual(struct.unpack("<III", self.receive(12)), (17, 5, 8))
        self.assertEqual(struct.unpack("<Q", self.receive(8)), (2,))

    def kick_pipe(self):
        read_fd, write_fd = os.pipe()
        # QEMU's macOS EventNotifier uses nonblocking pipes.
        os.set_blocking(read_fd, False)
        try:
            self.send(12, struct.pack("<Q", 0), read_fd)
        finally:
            os.close(read_fd)
        self.pipe_writers.add(write_fd)
        self.sync()
        return write_fd

    def queue_command(self, index=0, request=None):
        if request is None:
            request = struct.pack("<III", 256, 1, 256)
        self.memory[:16] = struct.pack("<QIHH", 0x1800, len(request), 1, 1)
        self.memory[16:32] = struct.pack("<QIHH", 0x1900, 48, 2, 0)
        self.memory[0x800:0x800 + len(request)] = request
        struct.pack_into("<H", self.memory, 0x204 + 2 * (index % 8), 0)
        struct.pack_into("<H", self.memory, 0x202, index + 1)

    def wait_used(self, expected, response=513):
        deadline = time.monotonic() + 2
        while struct.unpack_from("<H", self.memory, 0x402)[0] != expected:
            self.assertIsNone(self.process.poll(), "daemon exited processing kick")
            self.assertLess(time.monotonic(), deadline, "command was not processed")
            time.sleep(0.01)
        self.assertEqual(struct.unpack_from("<I", self.memory, 0x900)[0], response)

    def cpu_seconds(self):
        elapsed = subprocess.check_output(
            ["ps", "-o", "time=", "-p", str(self.process.pid)], text=True
        ).strip()
        minutes, seconds = elapsed.split(":")
        return int(minutes) * 60 + float(seconds)

    def assert_idle(self):
        self.sync()
        before = self.cpu_seconds()
        time.sleep(0.5)
        cpu = self.cpu_seconds() - before
        self.assertLess(cpu, 0.15, f"idle daemon burned {cpu:.2f}s CPU in 0.5s")

    def close_writer(self, fd):
        os.close(fd)
        self.pipe_writers.remove(fd)

    def test_idle_and_one_byte_kicks(self):
        writer = self.kick_pipe()
        self.assert_idle()
        self.queue_command()
        os.write(writer, b"\x01")
        self.wait_used(1)
        self.assert_idle()

    def test_closed_kick_stays_idle_and_can_be_replaced(self):
        writer = self.kick_pipe()
        self.close_writer(writer)
        self.assert_idle()
        writer = self.kick_pipe()
        self.queue_command()
        os.write(writer, b"\x01")
        self.wait_used(1)
        self.assert_idle()

    def test_final_kick_before_eof_is_processed(self):
        writer = self.kick_pipe()
        self.queue_command()
        os.write(writer, b"\x01")
        self.close_writer(writer)
        self.wait_used(1)
        self.assert_idle()

    def test_control_disconnect_exits(self):
        self.kick_pipe()
        self.conn.close()
        self.assertEqual(self.process.wait(timeout=2), 0)

    def test_commands_after_kick_eof(self):
        writer = self.kick_pipe()
        self.close_writer(writer)
        self.assert_idle()
        for index in range(10):
            self.queue_command(index)
            self.wait_used(index + 1)
        self.assert_idle()

    def test_commands_without_kick_fd(self):
        self.send(12, struct.pack("<Q", 0x100))
        self.assert_idle()
        self.queue_command()
        self.wait_used(1)
        self.assert_idle()

    def test_fallback_pauses_when_queue_disabled(self):
        writer = self.kick_pipe()
        self.close_writer(writer)
        self.assert_idle()
        self.send(18, struct.pack("<II", 0, 0))
        self.sync()
        self.queue_command()
        time.sleep(0.05)
        self.assertEqual(struct.unpack_from("<H", self.memory, 0x402)[0], 0)
        self.send(18, struct.pack("<II", 0, 1))
        self.wait_used(1)

    def test_get_vring_base_stops_fallback(self):
        writer = self.kick_pipe()
        self.close_writer(writer)
        self.assert_idle()
        self.send(11, struct.pack("<II", 0, 0))
        self.assertEqual(struct.unpack("<III", self.receive(12)), (11, 5, 8))
        self.assertEqual(struct.unpack("<II", self.receive(8)), (0, 0))
        self.queue_command()
        time.sleep(0.05)
        self.assertEqual(struct.unpack_from("<H", self.memory, 0x402)[0], 0)
        writer = self.kick_pipe()
        os.write(writer, struct.pack("<Q", 1))
        self.wait_used(1)

    @unittest.skipUnless(shutil.which("ffmpeg"), "ffmpeg required for H.264 fixture")
    def test_pipelined_hardware_streams_keep_output_ownership_across_ring_reuse(self):
        writer = self.kick_pipe()
        self.close_writer(writer)
        cursor = 0

        def resource(stream, queue, resource_id, address, length):
            request = bytearray(104)
            struct.pack_into("<IIII", request, 0, 260, stream, queue, resource_id)
            struct.pack_into("<I", request, 20, 1)
            struct.pack_into("<I", request, 56, 1)
            struct.pack_into("<QI", request, 88, address, length)
            return request

        def queue_resource(stream, queue, resource_id, timestamp, size=0):
            request = bytearray(64)
            struct.pack_into("<IIIIQ", request, 0, 261, stream, queue, resource_id, timestamp)
            struct.pack_into("<II", request, 24, 1, size)
            return request

        def destroy(stream, queue):
            return struct.pack("<IIII", 262, stream, queue, 0)

        def control(request):
            nonlocal cursor
            self.queue_command(cursor, request)
            cursor += 1
            self.wait_used(cursor, 512)

        def batch(commands):
            nonlocal cursor
            # Publish all four chains together, exactly like concurrent kernel
            # output/input pairs. Reuse these heads only after every used entry.
            for slot, (request, _response) in enumerate(commands):
                req_offset = 0x800 + 0x100 * slot
                resp_offset = 0xC00 + 0x80 * slot
                self.memory[req_offset:req_offset + len(request)] = request
                self.memory[resp_offset:resp_offset + 48] = bytes(48)
                struct.pack_into("<QIHH", self.memory, 32 * slot,
                                 0x1000 + req_offset, len(request), 1, 2 * slot + 1)
                struct.pack_into("<QIHH", self.memory, 32 * slot + 16,
                                 0x1000 + resp_offset, 48, 2, 0)
                struct.pack_into("<H", self.memory, 0x204 + 2 * ((cursor + slot) % 8), 2 * slot)
            cursor += len(commands)
            struct.pack_into("<H", self.memory, 0x202, cursor)
            deadline = time.monotonic() + 5
            while struct.unpack_from("<H", self.memory, 0x402)[0] != cursor:
                self.assertIsNone(self.process.poll(), "daemon exited during concurrent streams")
                self.assertLess(time.monotonic(), deadline, "pipelined commands did not complete")
                time.sleep(0.005)
            for slot, (request, response) in enumerate(commands):
                offset = 0xC00 + 0x80 * slot
                self.assertEqual(struct.unpack_from("<II", self.memory, offset),
                                 (response, struct.unpack_from("<I", request, 4)[0]))
                if response == 514:
                    self.assertEqual(struct.unpack_from("<Q", self.memory, offset + 8)[0],
                                     struct.unpack_from("<Q", request, 16)[0])

        streams = []
        for stream, color, width, height, input_addr, output_addr, yuv in [
            (2, "red", 128, 96, 0x5000, 0x9000, (81, 90, 240)),
            (4, "blue", 160, 112, 0x19000, 0x21000, (41, 240, 110)),
        ]:
            encoded = subprocess.check_output([
                "ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color={color}:s={width}x{height}",
                "-frames:v", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
                "-tune", "zerolatency", "-f", "h264", "pipe:1",
            ], timeout=10)
            self.assertLess(len(encoded), 8192)
            self.memory[input_addr - 0x1000:input_addr - 0x1000 + len(encoded)] = encoded
            streams.append((stream, width, height, input_addr, output_addr, encoded, yuv))

        def create(item):
            stream, _w, _h, input_addr, output_addr, _encoded, _yuv = item
            control(struct.pack("<IIIII", 257, stream, 0, 0, 4098) + bytes(68))
            control(resource(stream, 256, 1, input_addr, 8192))
            control(resource(stream, 257, 2, output_addr, 32768))

        def pixels(item):
            _stream, width, height, _input, address, _encoded, expected = item
            offset = address - 0x1000
            self.assertEqual(self.memory[offset:offset + 4], b"SVF1")
            self.assertEqual(struct.unpack_from("<IIII", self.memory, offset + 4),
                             (width, height, 0x34323076, width * height * 3 // 2))
            for index, component in [(0, expected[0]), (width * height, expected[1]),
                                     (width * height + 1, expected[2])]:
                self.assertLessEqual(abs(self.memory[offset + 20 + index] - component), 3)

        for item in streams:
            create(item)
        for frame in range(12):
            commands = []
            for stream, _w, _h, _input, _output, encoded, _yuv in streams:
                commands += [(queue_resource(stream, 257, 2, frame + 1), 514),
                             (queue_resource(stream, 256, 1, frame + 1, len(encoded)), 514)]
            batch(commands)
            for item in streams:
                pixels(item)
            if frame == 4:
                # A control teardown for stream 2 is interleaved between the
                # other stream's output/input commands. It must not consume or
                # remove stream 4's queued output resource or decoder.
                batch([(queue_resource(4, 257, 2, 99), 514),
                       (destroy(2, 256), 512),
                       (queue_resource(4, 256, 1, 99, len(streams[1][5])), 514),
                       (destroy(2, 257), 512)])
                pixels(streams[1])
                create(streams[0])
        self.assert_idle()

    @unittest.skipUnless(shutil.which("ffmpeg"), "ffmpeg required for H.264 fixture")
    def test_hardware_h264_decode_after_kick_eof(self):
        encoded = subprocess.check_output([
            "ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=red:s=128x96",
            "-frames:v", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
            "-tune", "zerolatency", "-f", "h264", "pipe:1",
        ], timeout=10)
        self.assertLess(len(encoded), 4096)
        self.memory[0x2000:0x2000 + len(encoded)] = encoded
        writer = self.kick_pipe()
        self.close_writer(writer)
        self.assert_idle()

        def resource(queue_type, resource_id, address, length):
            request = bytearray(104)
            struct.pack_into("<IIII", request, 0, 260, 1, queue_type, resource_id)
            struct.pack_into("<I", request, 20, 1)
            struct.pack_into("<I", request, 56, 1)
            struct.pack_into("<QI", request, 88, address, length)
            return request

        def queue_resource(queue_type, resource_id, size):
            request = bytearray(64)
            struct.pack_into("<IIIIQ", request, 0, 261, 1, queue_type, resource_id, 42)
            struct.pack_into("<I", request, 28, size)
            return request

        commands = [
            (struct.pack("<IIIII", 257, 1, 0, 0, 4098) + bytes(68), 512),
            (resource(256, 1, 0x3000, 4096), 512),
            (resource(257, 2, 0x5000, 32768), 512),
            (queue_resource(257, 2, 0), 514),
            (queue_resource(256, 1, len(encoded)), 514),
        ]
        for index, (request, response) in enumerate(commands):
            self.queue_command(index, request)
            self.wait_used(index + 1, response)

        self.assertEqual(self.memory[0x4000:0x4004], b"SVF1")
        self.assertEqual(struct.unpack_from("<IIII", self.memory, 0x4004),
                         (128, 96, 0x34323076, 128 * 96 * 3 // 2))
        pixels = self.memory[0x4014:0x4014 + 128 * 96 * 3 // 2]
        # Check the decoded red frame, not just a successful command response.
        self.assertLessEqual(abs(pixels[0] - 81), 3)
        self.assertLessEqual(abs(pixels[128 * 96] - 90), 3)
        self.assertLessEqual(abs(pixels[128 * 96 + 1] - 240), 3)
        self.assert_idle()


if __name__ == "__main__":
    if len(sys.argv) > 1:
        BINARY = Path(sys.argv.pop(1)).resolve()
    unittest.main()
