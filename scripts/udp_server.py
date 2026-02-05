#!/usr/bin/env python3
"""UDP server for testing Scarlet UDP client

Usage: python3 udp_server.py [host] <port>
Example: python3 udp_server.py 10.0.2.2 18080
         python3 udp_server.py 0.0.0.0 18080
         python3 udp_server.py 18080
"""

import sys
import socket

def main():
    if len(sys.argv) < 2 or len(sys.argv) > 3:
        print(f"Usage: {sys.argv[0]} [host] <port>")
        print(f"Example: {sys.argv[0]} 10.0.2.2 18080")
        print(f"         {sys.argv[0]} 0.0.0.0 18080")
        print(f"         {sys.argv[0]} 18080")
        return 1

    if len(sys.argv) == 2:
        host = "0.0.0.0"
        port = int(sys.argv[1])
    else:
        host = sys.argv[1]
        port = int(sys.argv[2])

    print(f"[UDP Server] Listening on {host}:{port}...")

    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
        sock.bind((host, port))
        print(f"[UDP Server] Bound to {host}:{port}")

        while True:
            data, addr = sock.recvfrom(1024)
            print(f"[UDP Server] Received from {addr[0]}:{addr[1]}: {data.decode('utf-8')}")

            response = f"[UDP Server] {data.decode('utf-8')}"
            sock.sendto(response.encode('utf-8'), addr)

    except KeyboardInterrupt:
        print(f"\n[UDP Server] Shutting down...")
    except socket.error as e:
        print(f"[UDP Server] Error: {e}")
        return 1

if __name__ == '__main__':
    sys.exit(main())
