#!/usr/bin/env python3
"""
Simple TCP Echo Server for testing Scarlet TCP client
Run on host machine (not inside Scarlet/QEMU)

Usage:
    python3 tcp_server.py [port]

Example:
    python3 tcp_server.py 8080

The server will:
1. Listen on all interfaces (0.0.0.0)
2. Accept one connection at a time
3. Echo back any received data
4. Print hex dump of received data
"""

import socket
import sys
import binascii


def run_server(port: int):
    """Run TCP echo server on specified port"""
    # Create socket with context manager
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)

        # Bind to all interfaces
        server.bind(('0.0.0.0', port))
        server.listen(1)

        print(f"[tcp-server] Listening on 0.0.0.0:{port}")
        print(f"[tcp-server] Ready to accept connections from Scarlet...")

        try:
            while True:
                # Accept connection
                client, addr = server.accept()
                print(f"[tcp-server] Client connected from {addr}")

                try:
                    # Receive data
                    data = client.recv(1024)
                    if data:
                        print(f"[tcp-server] Received {len(data)} bytes")
                        print(f"[tcp-server] Data (hex): {binascii.hexlify(data).decode()}")
                        print(f"[tcp-server] Data (text): {data.decode('utf-8', errors='replace')}")

                        # Echo back
                        client.sendall(data)
                        print(f"[tcp-server] Echoed back {len(data)} bytes")
                    else:
                        print(f"[tcp-server] Client closed connection")

                except Exception as e:
                    print(f"[tcp-server] Error: {e}")
                finally:
                    client.close()
                    print(f"[tcp-server] Client disconnected")

        except KeyboardInterrupt:
            print(f"\n[tcp-server] Shutting down...")


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    run_server(port)
