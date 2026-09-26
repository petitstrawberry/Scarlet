#!/usr/bin/env python3
"""Build local Scarlet and exercise dual-NIC link lifecycle via QMP (no SD writes)."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

sys.dont_write_bytecode = True
from qmp import Qmp

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / '.build/network-management'


def run(args, log, **kwargs):
    with (OUT / log).open('w') as output:
        subprocess.run(args, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, check=True, **kwargs)


def cpio(entries, output):
    data = bytearray()
    for inode, (name, mode, payload) in enumerate(entries + [('TRAILER!!!', 0, b'')], 1):
        name = name.encode() + b'\0'
        fields = [inode, mode, 0, 0, 1, 0, len(payload), 0, 0, 0, 0, len(name), 0]
        data.extend(b'070701' + ''.join(f'{value:08x}' for value in fields).encode())
        data.extend(name)
        data.extend(b'\0' * (-len(data) % 4))
        data.extend(payload)
        data.extend(b'\0' * (-len(data) % 4))
    output.write_bytes(data)


def build():
    fixture = OUT / 'kernel-fixture'
    (fixture / 'src').mkdir(parents=True, exist_ok=True)
    (fixture / 'Cargo.toml').write_text(f'''[package]
name = "network-qa-kernel"
version = "0.1.0"
edition = "2024"
[dependencies]
scarlet = {{ path = "{ROOT / 'kernel'}", default-features = false, features = ["linux-boot", "network", "user-fpu", "user-vector"] }}
[profile.release]
panic = "abort"
''')
    (fixture / 'src/main.rs').write_text('''#![no_std]
#![no_main]
#[used]
static ENTRY: extern "C" fn() -> ! = scarlet::arch::aarch64::boot::linux::image_head;
''')
    target = json.loads((ROOT / 'kernel/targets/aarch64-unknown-none-elf.json').read_text())
    target.pop('pre-link-args', None)
    (fixture / 'aarch64-network-none.json').write_text(json.dumps(target))
    env = os.environ.copy()
    env['RUSTFLAGS'] = f'-C link-arg=-T{ROOT / "guest_tests/network_management/kernel.ld"}'
    run(['cargo', 'build', '--manifest-path', str(fixture / 'Cargo.toml'), '--release',
         '--target', str(fixture / 'aarch64-network-none.json'), '-Z', 'build-std=core,alloc,compiler_builtins'],
        'kernel-build.log', env=env)
    subprocess.run(['llvm-objcopy', '-O', 'binary', str(fixture / 'target/aarch64-network-none/release/network-qa-kernel'),
                    str(OUT / 'Image')], check=True)
    env = os.environ.copy()
    env['CARGO_TARGET_DIR'] = str(OUT / 'userspace-target')
    run(['cargo', 'build', '--manifest-path', 'user/bin/Cargo.toml', '--no-default-features',
         '--bin', 'init', '--bin', 'netcfgd', '--release', '--target', 'aarch64-unknown-scarlet'], 'init-build.log', env=env)
    run(['cargo', 'build', '--manifest-path', 'guest_tests/network_management/Cargo.toml',
         '--release', '--target', 'aarch64-unknown-scarlet'], 'qa-build.log')
    entries = [(name, 0o40755, b'') for name in ['bin', 'dev', 'etc', 'etc/netcfgd.d', 'tmp', 'root', 'mnt', 'mnt/newroot']]
    for name, path in [
        ('init', OUT / 'userspace-target/aarch64-unknown-scarlet/release/init'),
        ('bin/netcfgd', OUT / 'userspace-target/aarch64-unknown-scarlet/release/netcfgd'),
        ('bin/network-qa', ROOT / 'guest_tests/network_management/target/aarch64-unknown-scarlet/release/network-management-qa')]:
        entries.append((name, 0o100755, path.read_bytes()))
    config = '''[network]
dhcp_timeout_ms = 500
dhcp_attempts = 3
[[interface]]
name = "veth0"
method = "dhcp"
metric = 10
[[interface]]
name = "veth1"
method = "dhcp"
metric = 100
'''
    entries.append(('etc/netcfgd.d/10-test.toml', 0o100644, config.encode()))
    cpio(entries, OUT / 'initramfs.cpio')


def exercise():
    qmp_path = OUT / 'qmp.sock'
    qmp_path.unlink(missing_ok=True)
    log = OUT / 'qemu.log'
    command = ['qemu-system-aarch64', '-machine', 'virt,gic-version=3', '-cpu', 'max', '-m', '2G', '-smp', '2',
               '-global', 'virtio-mmio.force-legacy=false', '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot',
               '-qmp', f'unix:{qmp_path},server=on,wait=off', '-kernel', str(OUT / 'Image'),
               '-initrd', str(OUT / 'initramfs.cpio'), '-append', 'console=ttyAMA0 init.exec=/bin/network-qa']
    # virtio-mmio slots enumerate in reverse creation order on QEMU virt.
    for index in reversed(range(2)):
        command += ['-netdev', f'user,id=net{index},net=10.0.{index+2}.0/24',
                    '-device', f'virtio-net-device,id=nic{index},netdev=net{index},mac=52:54:00:12:34:0{index}']
    with log.open('w') as stream:
        process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT)
        def wait(marker):
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                content = log.read_text(errors='replace')
                if marker in content:
                    print(marker, flush=True)
                    return
                if process.poll() is not None or 'panicked' in content or 'Panic occurred:' in content or '[Scarlet Kernel] panic:' in content:
                    raise RuntimeError(f'guest failed waiting for {marker}; see {log}')
                time.sleep(0.1)
            raise TimeoutError(f'waiting for {marker}; see {log}')
        try:
            wait('NETWORK_QA INITIAL')
            with Qmp(str(qmp_path)) as qmp:
                qmp.execute('set_link', {'name': 'nic0', 'up': False})
                wait('NETWORK_QA DOWN')
                qmp.execute('set_link', {'name': 'nic0', 'up': True})
                wait('NETWORK_QA RESTORED')
                for nic in ['nic0', 'nic1']:
                    qmp.execute('set_link', {'name': nic, 'up': False})
                wait('NETWORK_QA ALL_DOWN')
                qmp.execute('set_link', {'name': 'nic1', 'up': True})
                wait('NETWORK_QA PASS')
            process.wait(timeout=10)
            if process.returncode != 0:
                raise RuntimeError(f'QEMU exited {process.returncode}')
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true')
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    if not args.no_build:
        build()
    exercise()

if __name__ == '__main__':
    main()
