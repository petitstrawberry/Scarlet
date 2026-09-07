#!/usr/bin/env python3
"""Send one bounded QMP command to a local Scarlet QEMU instance."""

import argparse
import json
import math
import socket
import time


class Qmp:
    """Local QMP connection with capability negotiation and response IDs."""

    def __init__(self, path, timeout=10.0):
        if not math.isfinite(timeout) or timeout <= 0:
            raise ValueError("timeout must be finite and positive")
        self.timeout = timeout
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.pending = b""
        self.next_id = 0
        try:
            self.sock.settimeout(timeout)
            self.sock.connect(path)
            greeting = self._read(time.monotonic() + timeout)
            if "QMP" not in greeting:
                raise RuntimeError("QMP greeting missing")
            self.execute("qmp_capabilities")
        except Exception:
            self.close()
            raise

    def _read(self, deadline):
        if time.monotonic() >= deadline:
            raise TimeoutError("QMP response deadline exceeded")
        while b"\n" not in self.pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("QMP response deadline exceeded")
            self.sock.settimeout(remaining)
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError("QMP disconnected before sending a response")
            self.pending += chunk
        line, self.pending = self.pending.split(b"\n", 1)
        message = json.loads(line)
        if not isinstance(message, dict):
            raise RuntimeError("QMP response is not an object")
        return message

    def execute(self, command, arguments=None):
        """Execute one command, ignoring asynchronous events while awaiting its ID."""
        if arguments is not None and not isinstance(arguments, dict):
            raise ValueError("QMP arguments must be a JSON object")
        self.next_id += 1
        request_id = self.next_id
        request = {"execute": command, "id": request_id}
        if arguments is not None:
            request["arguments"] = arguments
        deadline = time.monotonic() + self.timeout
        self.sock.settimeout(self.timeout)
        self.sock.sendall(json.dumps(request).encode() + b"\r\n")
        while True:
            response = self._read(deadline)
            if response.get("id") != request_id:
                continue
            if "error" in response:
                raise RuntimeError(f"QMP {command} failed: {response['error']}")
            if "return" not in response:
                raise RuntimeError(f"QMP {command} response has no result")
            return response["return"]

    def close(self):
        """Release the socket without terminating or resetting the guest."""
        self.sock.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True, help="local Unix QMP socket")
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("command", nargs="?", default="query-status")
    parser.add_argument("arguments", nargs="?", default="{}", type=json.loads)
    args = parser.parse_args()
    try:
        with Qmp(args.socket, args.timeout) as qmp:
            print(json.dumps(qmp.execute(args.command, args.arguments)))
    except (OSError, EOFError, ValueError, RuntimeError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
