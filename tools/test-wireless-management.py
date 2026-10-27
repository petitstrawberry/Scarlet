#!/usr/bin/env python3
"""Exercise native Wi-Fi handles, poll and both control-only mock modes in QEMU."""
import argparse
import importlib.util
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('network_management', ROOT / 'tools/test-network-management.py')
network = importlib.util.module_from_spec(spec)
spec.loader.exec_module(network)
OUT = ROOT / '.build/wireless-management'
network.OUT = OUT


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true')
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    if not args.no_build:
        network.build(wireless=True)
    log = OUT / 'qemu.log'
    command = ['qemu-system-aarch64', '-machine', 'virt,gic-version=3', '-cpu', 'max', '-m', '2G', '-smp', '2',
               '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot', '-kernel', str(OUT / 'Image'),
               '-initrd', str(OUT / 'initramfs.cpio'), '-append', 'console=ttyAMA0 wifi.mock=1 init.exec=/bin/network-qa']
    with log.open('w') as stream:
        process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                content = log.read_text(errors='replace')
                if 'WIRELESS_QA PASS' in content:
                    process.wait(timeout=10)
                    if process.returncode != 0:
                        raise RuntimeError(f'QEMU exited {process.returncode}; see {log}')
                    print('WIRELESS_QA PASS', flush=True)
                    return
                if process.poll() is not None or any(x in content for x in ('panicked', 'Panic occurred:', '[Scarlet Kernel] panic:')):
                    raise RuntimeError(f'wireless guest failed; see {log}')
                time.sleep(0.1)
            raise TimeoutError(f'wireless guest timed out; see {log}')
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    main()
