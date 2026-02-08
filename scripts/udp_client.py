#!/usr/bin/env python3
"""UDP client for testing Scarlet UDP server

Usage: python3 udp_client.py <host> <port>
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

    print(f"[UDP Client] Connecting to {host}:{port}...")

    try:
        # Create UDP socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)

        # Send a message
        message = "Hello from Python UDP client!"
        print(f"[UDP Client] Sending to {host}:{port}: {message}")
        sock.sendto(message.encode('utf-8'), (host, port))

        # Receive response
        data, addr = sock.recvfrom(1024)
        print(f"[UDP Client] Received from {addr[0]}:{addr[1]}: {data.decode('utf-8')}")

        # Close socket
        sock.close()
        print(f"[UDP Client] Connection closed")
        return 0

    except socket.error as e:
        print(f"[UDP Client] Error: {e}")
        return 1

if __name__ == '__main__':
    sys.exit(main())
