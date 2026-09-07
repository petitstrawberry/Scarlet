"""Transport tests that do not boot a guest or need a display server."""

import json
import socket
import threading
import unittest
from unittest.mock import patch

from qmp import Qmp


class QmpTests(unittest.TestCase):
    def connect(self, reply):
        client, server = socket.socketpair()
        self.addCleanup(client.close)
        self.addCleanup(server.close)
        client_proxy = unittest.mock.Mock(wraps=client)
        client_proxy.connect.return_value = None
        errors = []

        def serve():
            try:
                with server.makefile("rwb", buffering=0) as stream:
                    stream.write(b'{"QMP": {}}\r\n')
                    capabilities = json.loads(stream.readline())
                    if capabilities["execute"] != "qmp_capabilities":
                        raise AssertionError("capabilities must be negotiated first")
                    response = {"return": {}, "id": capabilities["id"]}
                    stream.write(json.dumps(response).encode() + b"\r\n")
                    line = stream.readline()
                    if line:
                        reply(stream, json.loads(line))
            except Exception as error:
                errors.append(error)
            finally:
                server.close()

        worker = threading.Thread(target=serve, daemon=True)
        worker.start()

        def finish_worker():
            worker.join(1)
            self.assertFalse(worker.is_alive(), "fake QMP server did not stop")
            self.assertEqual(errors, [], "fake QMP server failed")

        self.addCleanup(finish_worker)
        with patch("qmp.socket.socket", return_value=client_proxy):
            return Qmp("unused-test-path", timeout=1)

    def test_negotiates_and_ignores_events_and_unrelated_ids(self):
        def reply(stream, request):
            stream.write(b'{"event":"STOP"}\r\n')
            stream.write(b'{"return":"unrelated","id":99}\r\n')
            response = {"return": request["arguments"], "id": request["id"]}
            stream.write(json.dumps(response).encode() + b"\r\n")

        with self.connect(reply) as qmp:
            self.assertEqual(qmp.execute("query-test", {"value": 42}), {"value": 42})

    def test_command_error_is_not_success(self):
        def reply(stream, request):
            response = {"error": {"desc": "unsupported"}, "id": request["id"]}
            stream.write(json.dumps(response).encode() + b"\r\n")

        with self.connect(reply) as qmp:
            with self.assertRaisesRegex(RuntimeError, "unsupported"):
                qmp.execute("missing-command")

    def test_disconnect_is_not_success(self):
        with self.connect(lambda *_: None) as qmp:
            with self.assertRaises(EOFError):
                qmp.execute("query-test")

    def test_invalid_timeout_is_rejected(self):
        for timeout in [0, -1, float("inf"), float("nan")]:
            with self.subTest(timeout=timeout):
                with self.assertRaises(ValueError):
                    Qmp("unused-test-path", timeout=timeout)

    def test_arguments_must_be_an_object(self):
        with self.connect(lambda *_: None) as qmp:
            with self.assertRaises(ValueError):
                qmp.execute("query-test", [])

    def test_silent_peer_has_a_bounded_wait(self):
        release = threading.Event()

        def reply(*_):
            release.wait(2)

        with self.connect(reply) as qmp:
            qmp.timeout = 0.05
            try:
                with self.assertRaises(TimeoutError):
                    qmp.execute("query-test")
            finally:
                release.set()

    def test_partial_messages_cannot_extend_the_deadline(self):
        release = threading.Event()

        def reply(stream, _):
            for _ in range(100):
                if release.wait(0.01):
                    return
                try:
                    stream.write(b" ")
                except BrokenPipeError:
                    return

        with self.connect(reply) as qmp:
            qmp.timeout = 0.1
            try:
                with self.assertRaises(TimeoutError):
                    qmp.execute("query-test")
            finally:
                release.set()


if __name__ == "__main__":
    unittest.main()
