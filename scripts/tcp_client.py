#!/usr/bin/env python3
"""TCP client for testing Scarlet TCP server

Usage: python3 tcp_client.py <host> <port>
"""

import sys
import socket

def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <host> <port>")
        print(f"Example: {sys.argv[0]} 10.0.2.15 8080")
        return 1

    host = sys.argv[1]
    port = int(sys.argv[2])

    print(f"[TCP Client] Connecting to {host}:{port}...")

    try:
        # Create TCP socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP)

        # Connect to server
        sock.connect((host, port))
        print(f"[TCP Client] Connected to {host}:{port}")

        # Send a message
        message = "Hello from Python client!"
        sock.sendall(message.encode('utf-8'))
        print(f"[TCP Client] Sent: {message}")

        # Receive response
        data = sock.recv(1024)
        print(f"[TCP Client] Received: {data.decode('utf-8')}")

        # Close connection
        sock.close()
        print(f"[TCP Client] Connection closed")

        return 0

    except socket.error as e:
        print(f"[TCP Client] Connection failed: {e}")
        return 1

if __name__ == '__main__':
    sys.exit(main())
