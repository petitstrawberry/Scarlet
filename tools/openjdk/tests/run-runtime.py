#!/usr/bin/env python3
"""Run the native runtime prerequisite probe in an isolated, diskless VM."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def archive(entries, path):
    data = bytearray()
    for index, (name, mode, payload) in enumerate(entries + [('TRAILER!!!', 0, b'')], 1):
        name = name.encode() + b'\0'
        fields = [index, mode, 0, 0, 1, 0, len(payload), 0, 0, 0, 0, len(name), 0]
        data.extend(b'070701' + ''.join(f'{x:08x}' for x in fields).encode() + name)
        data.extend(b'\0' * (-len(data) % 4))
        data.extend(payload)
        data.extend(b'\0' * (-len(data) % 4))
    path.write_bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('kernel', 'init', 'probe', 'output'):
        parser.add_argument('--'+name, required=True, type=Path)
    parser.add_argument('--accel', choices=('tcg', 'hvf'), default='tcg')
    parser.add_argument('--cpus', type=int, default=4)
    parser.add_argument('--timeout', type=int, default=90)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    initrd = args.output/'initramfs.cpio'
    entries = [(name, 0o40755, b'') for name in ('bin', 'dev', 'etc', 'tmp', 'root', 'mnt', 'mnt/newroot')]
    entries += [('init', 0o100755, args.init.read_bytes()), ('bin/runtime-probe', 0o100755, args.probe.read_bytes())]
    archive(entries, initrd)
    command = ['qemu-system-aarch64', '-machine', 'virt,gic-version=3', '-accel', args.accel,
               '-cpu', 'host' if args.accel == 'hvf' else 'max', '-m', '1G', '-smp', str(args.cpus),
               '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot',
               '-kernel', str(args.kernel), '-initrd', str(initrd),
               '-append', 'console=ttyAMA0 init.exec=/bin/runtime-probe']
    with (args.output/'qemu.log').open('w') as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=args.timeout)
    content = (args.output/'qemu.log').read_text(errors='replace')
    if result.returncode or 'OPENJDK_RUNTIME FAIL' in content or 'OPENJDK_RUNTIME PASS' not in content:
        raise SystemExit('Native runtime probe failed; see '+str(args.output/'qemu.log'))
    def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
    receipt = {'result': 'PASS', 'scope': 'native prerequisites only; not JVM execution',
               'command': command, 'kernel_sha256': sha(args.kernel), 'init_sha256': sha(args.init),
               'probe_sha256': sha(args.probe), 'log_sha256': sha(args.output/'qemu.log')}
    (args.output/'validation.json').write_text(json.dumps(receipt, indent=2)+'\n')
    print('Native OpenJDK runtime prerequisites PASS')

if __name__ == '__main__':
    main()
