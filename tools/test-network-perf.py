#!/usr/bin/env python3
"""Finite, content-checked TCP transfers through local Scarlet in QEMU."""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'tools/network-perf'


def run(command, log, **kwargs):
    with log.open('w') as output:
        subprocess.run(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT,
                       check=True, **kwargs)


def cpio(entries, output):
    data = bytearray()
    for inode, (name, mode, payload) in enumerate(entries + [('TRAILER!!!', 0, b'')], 1):
        name = name.encode() + b'\0'
        fields = [inode, mode, 0, 0, 1, 0, len(payload), 0, 0, 0, 0, len(name), 0]
        data.extend(b'070701' + ''.join(f'{v:08x}' for v in fields).encode())
        data.extend(name)
        data.extend(b'\0' * (-len(data) % 4))
        data.extend(payload)
        data.extend(b'\0' * (-len(data) % 4))
    output.write_bytes(data)


def source_hashes():
    return {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
            for directory in [ROOT / 'kernel/src', SOURCE] for p in directory.rglob('*')
            if p.is_file() and p.suffix in ('.rs', '.ld', '.toml', '.lock')}


def build(out):
    hashes = source_hashes()
    fixture = out / 'kernel-fixture'
    (fixture / 'src').mkdir(parents=True, exist_ok=True)
    (fixture / 'Cargo.toml').write_text(f'''[package]
name = "network-perf-kernel"
version = "0.1.0"
edition = "2024"
[workspace]
[dependencies]
scarlet = {{ path = "{ROOT / 'kernel'}", default-features = false, features = ["linux-boot", "network", "user-fpu", "user-vector"] }}
[profile.release]
panic = "abort"
''')
    (fixture / 'src/main.rs').write_text((SOURCE / 'kernel.rs').read_text())
    target = json.loads((ROOT / 'kernel/targets/aarch64-unknown-none-elf.json').read_text())
    target.pop('pre-link-args', None)
    target['features'] = '+v8a,+strict-align,-neon,-lse'
    target['cpu'] = 'cortex-a57'
    (fixture / 'aarch64-network-perf.json').write_text(json.dumps(target))
    env = os.environ.copy()
    env['RUSTFLAGS'] = f'-C link-arg=-T{SOURCE / "kernel.ld"}'
    run(['cargo', 'build', '--manifest-path', str(fixture / 'Cargo.toml'), '--release',
         '--target', str(fixture / 'aarch64-network-perf.json'), '-Z', 'build-std=core,alloc,compiler_builtins',
         '-Z', 'build-std-features=compiler-builtins-mem'],
        out / 'kernel-build.log', env=env)
    subprocess.run(['llvm-objcopy', '-O', 'binary', str(fixture / 'target/aarch64-network-perf/release/network-perf-kernel'),
                    str(out / 'Image')], check=True)
    env = os.environ.copy()
    env['CARGO_TARGET_DIR'] = str(out / 'userspace-target')
    run(['cargo', 'build', '--manifest-path', 'user/bin/Cargo.toml', '--no-default-features',
         '--bin', 'init', '--release', '--target', 'aarch64-unknown-scarlet'], out / 'init-build.log', env=env)
    run(['cargo', 'build', '--manifest-path', str(SOURCE / 'guest/Cargo.toml'), '--release',
         '--target', 'aarch64-unknown-scarlet'], out / 'guest-build.log', env=env)
    run(['cargo', 'build', '--manifest-path', 'user/scarlet-ld/Cargo.toml', '--release',
         '--target', 'aarch64-unknown-scarlet'], out / 'loader-build.log', env=env)
    entries = [(name, 0o40755, b'') for name in ['bin', 'dev', 'etc', 'tmp', 'root', 'mnt', 'mnt/newroot']]
    for name, binary in [('init', 'init'), ('bin/network-perf', 'network-perf-guest'), ('bin/scarlet-ld', 'scarlet-ld')]:
        entries.append((name, 0o100755, (out / 'userspace-target/aarch64-unknown-scarlet/release' / binary).read_bytes()))
    cpio(entries, out / 'initramfs.cpio')
    if hashes != source_hashes():
        raise RuntimeError('source changed during build; rebuild before measuring')
    (out / 'build-source.json').write_text(json.dumps(hashes, indent=2) + '\n')


def exact(stream, size):
    result = bytearray()
    while len(result) < size:
        data = stream.recv(size - len(result))
        if not data:
            raise RuntimeError(f'short receipt: {len(result)}/{size}')
        result.extend(data)
    return bytes(result)


def cpu_seconds(pid):
    text = subprocess.check_output(['ps', '-p', str(pid), '-o', 'time='], text=True).strip()
    values = text.split(':')
    return sum(float(v) * 60 ** i for i, v in enumerate(reversed(values)))


def exercise(out, artifacts, mode, args):
    directory = out / args.label / mode
    directory.mkdir(parents=True, exist_ok=True)
    log = directory / 'qemu.log'
    port = args.port
    command = [args.qemu, '-machine', 'virt,gic-version=3', '-cpu', 'host' if args.accel == 'hvf' else 'cortex-a57',
               '-accel', args.accel, '-m', '2G', '-smp', str(args.cpus), '-display', 'none', '-monitor', 'none',
               '-serial', 'stdio', '-no-reboot', '-global', 'virtio-mmio.force-legacy=false',
               '-kernel', str(artifacts / 'Image'), '-initrd', str(artifacts / 'initramfs.cpio'),
               '-append', 'maxcpus=4 init.exec=/bin/network-perf',
               '-netdev', f'user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:8081']
    if args.capture:
        command += ['-object', f'filter-dump,id=capture,netdev=net0,file={directory / "network.pcap"}']
    if mode == 'virtio':
        command += ['-device', 'virtio-net-device,netdev=net0,mac=52:54:00:12:34:56']
    else:
        # Direct Image boot has no UEFI PCI allocator. Assign this fixture's
        # explicitly selected slot-1 BAR in the virt machine's PCI MMIO range.
        command += ['-device', 'qemu-xhci,id=xhci,bus=pcie.0,addr=0x1',
                    '-device', 'usb-ncm,id=ncm0,bus=xhci.0,netdev=net0,mac=52:54:00:12:34:57']
    results = []
    with log.open('w') as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT, cwd=ROOT)
        try:
            deadline = time.monotonic() + 120
            while 'NETWORK_PERF READY' not in log.read_text(errors='replace'):
                content = log.read_text(errors='replace')
                if process.poll() is not None or time.monotonic() > deadline or any(
                    marker in content for marker in ('panicked', 'ELF loading failed', 'Panic occurred:')):
                    raise RuntimeError(f'guest not ready: {log}')
                time.sleep(0.1)
            payload = bytes([0x5a]) * 65536
            for index in range(args.repeats):
                for direction in ('rx', 'tx'):
                    with socket.create_connection(('127.0.0.1', port), timeout=60) as stream:
                        stream.settimeout(60)
                        cpu_start = cpu_seconds(process.pid)
                        stream.sendall(struct.pack('<BBQ', direction == 'tx', args.profile, args.bytes))
                        assert exact(stream, 1) == b'R'
                        start = time.monotonic_ns()
                        remaining = args.bytes
                        while remaining:
                            size = min(remaining, len(payload))
                            if direction == 'rx':
                                stream.sendall(payload[:size])
                            else:
                                data = stream.recv(size)
                                if not data or data != payload[:len(data)]:
                                    raise RuntimeError('guest TX payload mismatch/short read')
                                size = len(data)
                            remaining -= size
                        if direction == 'tx':
                            stream.sendall(b'V')
                        assert struct.unpack('<Q', exact(stream, 8))[0] == args.bytes
                        elapsed = time.monotonic_ns() - start
                        cpu = cpu_seconds(process.pid) - cpu_start
                    result = dict(index=index, direction=direction, bytes=args.bytes, host_elapsed_ns=elapsed,
                                  host_mbps=args.bytes * 8000 / elapsed, qemu_process_cpu_seconds=cpu,
                                  payload_verified=True, profiling=args.profile)
                    results.append(result)
                    print(json.dumps(dict(mode=mode, **result)), flush=True)
                    time.sleep(0.1)
            content = log.read_text(errors='replace')
            guest = [dict(direction=d, bytes=int(b), calls=int(c), elapsed_ns=int(t), profile=int(p), busy_ns=int(busy), idle_ns=int(idle))
                     for d, b, c, t, p, busy, idle in re.findall(r'NETWORK_PERF RESULT direction=(rx|tx) bytes=(\d+) calls=(\d+) elapsed_ns=(\d+) profile=(\d+) busy_ns=(\d+) idle_ns=(\d+)', content)]
            assert len(guest) == len(results), (guest, results)
            for result, observed in zip(results, guest):
                assert result['direction'] == observed['direction'] and result['bytes'] == observed['bytes']
                result['guest'] = observed
            (directory / 'receipt.json').write_text(json.dumps(dict(mode=mode, command=command, results=results,
                kernel_sha256=hashlib.sha256((artifacts / 'Image').read_bytes()).hexdigest(),
                source_sha256=json.loads((artifacts / 'build-source.json').read_text()),
                qemu_version=subprocess.check_output([args.qemu, '--version'], text=True),
                runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                accelerator=args.accel, physical_throughput=False), indent=2) + '\n')
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true')
    parser.add_argument('--build-only', action='store_true')
    parser.add_argument('--artifacts', type=Path, help='Reuse a frozen Image/initramfs/build-source directory')
    parser.add_argument('--capture', action='store_true', help='Capture packets (adds host disk I/O)')
    parser.add_argument('--qemu', default=shutil.which('qemu-system-aarch64'),
                        help='QEMU binary (NCM requires the repository dev-shell QEMU)')
    parser.add_argument('--mode', choices=['virtio', 'ncm', 'both'], default='both')
    parser.add_argument('--bytes', type=int, default=4 * 1024 * 1024)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--cpus', type=int, choices=[1, 2, 4], default=4)
    parser.add_argument('--accel', choices=['hvf', 'tcg'],
                        default='hvf' if platform.system() == 'Darwin' and platform.machine() == 'arm64' else 'tcg')
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--port', type=int, default=18081)
    parser.add_argument('--label', default=time.strftime('%Y%m%d-%H%M%S'))
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', args.label):
        parser.error('label must be a simple directory name starting with a letter or digit')
    if not 0 < args.bytes <= 64 * 1024 * 1024 or not 0 < args.repeats <= 20:
        parser.error('bytes must be 1..64 MiB and repeats 1..20')
    if not args.qemu:
        parser.error('QEMU missing; run this tool inside nix develop')
    if args.accel not in subprocess.check_output([args.qemu, '-accel', 'help'], text=True):
        parser.error(f'{args.accel} accelerator unavailable in {args.qemu}')
    if args.mode != 'virtio':
        devices = subprocess.check_output([args.qemu, '-device', 'help'], text=True,
                                          stderr=subprocess.STDOUT)
        if 'usb-ncm' not in devices:
            parser.error('USB-NCM missing; use the QEMU from nix develop')
    out = ROOT / '.build/network-perf'
    out.mkdir(parents=True, exist_ok=True)
    if not args.no_build and args.artifacts is None:
        build(out)
    if args.build_only:
        return
    artifacts = out / args.label / 'artifacts'
    if (out / args.label).exists():
        parser.error('label already exists; choose a new label to preserve previous results')
    artifacts.mkdir(parents=True, exist_ok=True)
    for name in ('Image', 'initramfs.cpio', 'build-source.json'):
        shutil.copy2((args.artifacts or out) / name, artifacts / name)
    for mode in (['virtio', 'ncm'] if args.mode == 'both' else [args.mode]):
        exercise(out, artifacts, mode, args)


if __name__ == '__main__':
    main()
