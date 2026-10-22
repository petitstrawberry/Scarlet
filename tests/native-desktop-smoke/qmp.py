#!/usr/bin/env python3
"""Send one QMP request to the isolated desktop smoke instance."""
import argparse
import json
from pathlib import Path
import socket


def request(path, payload):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(10)
        connection.connect(str(path))
        stream = connection.makefile("rwb", buffering=0)
        json.loads(stream.readline())
        for command in [{"execute": "qmp_capabilities"}, payload]:
            stream.write(json.dumps(command).encode() + b"\r\n")
            while True:
                line = stream.readline()
                if not line:
                    raise RuntimeError("QMP closed before replying")
                result = json.loads(line)
                if "error" in result:
                    raise RuntimeError(result)
                if "return" in result:
                    break
        return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("socket", type=Path)
    parser.add_argument("request", help="JSON QMP request")
    args = parser.parse_args()
    print(json.dumps(request(args.socket, json.loads(args.request))))
