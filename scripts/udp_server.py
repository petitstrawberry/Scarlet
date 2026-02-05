#!/usr/bin/env python3
"""UDP server for testing Scarlet UDP client

Usage: python3 udp_server.py <host> <port>
"""

import sys
import socket

def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <host> <port>")
        print(f"Example: {sys.argv[0]} 10.0.2.15 18080")
        return 1

    host = sys.argv[1]
    port = int(sys.argv[2])

    print(f"[UDP Server] Listening on {host}:{port}...")

    try:
        # Create UDP socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)

        # Bind to port
        sock.bind((host, port))
        print(f"[UDP Server] Bound to {host}:{port}")

        while True:
            # Receive datagram
            data, addr = sock.recvfrom(1024)
            print(f"[UDP Server] Received from {addr[0]}:{addr[1]}: {data.decode('utf-8')}")

            # Echo response
            response = f"[UDP Server] {data.decode('utf-8')}"
            sock.sendto(response.encode('utf-8'), addr)

    except KeyboardInterrupt:
        print(f"\n[UDP Server] Shutting down...")
    except socket.error as e:
        print(f"[UDP Server] Error: {e}")
        return 1

if __name__ == '__main__':
    sys.exit(main())
