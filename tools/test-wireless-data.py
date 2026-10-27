#!/usr/bin/env python3
"""QEMU Wi-Fi controlled-port lifecycle over a real virtio/slirp UDP path."""
import argparse
import importlib.util
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
import socket
import struct
import subprocess
import sys
import time
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('network_management', ROOT / 'tools/test-network-management.py')
network = importlib.util.module_from_spec(spec)
spec.loader.exec_module(network)
OUT = ROOT / '.build/wireless-data'
network.OUT = OUT
from qmp import Qmp


def discovers(path):
    data = path.read_bytes()
    endian = '<' if data[:4] == b'\xd4\xc3\xb2\xa1' else '>'
    offset = 24
    xids = set()
    while offset + 16 <= len(data):
        _, _, size, _ = struct.unpack_from(endian + 'IIII', data, offset)
        offset += 16
        frame = data[offset:offset+size]
        offset += size
        if len(frame) < 42 or frame[12:14] != b'\x08\x00' or frame[23] != 17:
            continue
        udp = 14 + (frame[14] & 15) * 4
        if len(frame) < udp + 8 or frame[udp:udp+4] != b'\x00D\x00C':
            continue
        bootp = frame[udp+8:]
        if len(bootp) >= 244 and bootp[240:243] == b'\x35\x01\x01':
            xids.add(bootp[4:8].hex())
    return sorted(xids)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true')
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    sources = list((ROOT / 'kernel/src/device/wireless').glob('*.rs'))
    sources += [ROOT / path for path in (
        'kernel/src/device/network/mod.rs', 'kernel/src/drivers/network/virtio_net.rs',
        'kernel/src/network/mod.rs', 'kernel/src/network/ethernet.rs', 'kernel/src/network/ethernet_interface.rs',
        'kernel/src/network/arp.rs', 'kernel/src/network/packet.rs', 'kernel/src/network/link.rs',
        'kernel/src/network/config.rs', 'user/lib/scarlet-abi/src/wireless.rs',
        'user/lib/scarlet-os/src/wireless.rs', 'tools/wireless-data/guest/src/main.rs',
        'tools/wireless-data/guest/Cargo.toml', 'tools/test-wireless-data.py', 'tools/test-network-management.py')]
    sources += list((ROOT / 'user/bin/src/netcfgd').glob('*.rs'))
    def source_hashes():
        return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(sources)}
    before = source_hashes()
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('127.0.0.1', 0))
        port = server.getsockname()[1]
        port_entry = [('etc/wireless-data-port', 0o100644, str(port).encode())]
        if not args.no_build:
            network.build(wireless_data=True, extra_entries=port_entry)
        else:
            # Repackage the changing host port without recompilation.
            image = OUT / 'initramfs.cpio'
            # build()'s newc archive can be read without external cpio tools.
            raw = image.read_bytes(); offset = 0; entries = []
            while True:
                fields = [int(raw[offset+6+i*8:offset+14+i*8], 16) for i in range(13)]
                start = offset + 110; name = raw[start:start+fields[11]-1].decode()
                start = (start+fields[11]+3)&~3
                payload = raw[start:start+fields[6]]; offset = (start+fields[6]+3)&~3
                if name == 'TRAILER!!!': break
                if name != 'etc/wireless-data-port': entries.append((name,fields[1],payload))
            network.cpio(entries+port_entry,image)
        log = OUT / 'qemu.log'; pcap = OUT / 'veth0.pcap'
        qmp_path = OUT / 'qmp.sock'; qmp_path.unlink(missing_ok=True)
        command = ['qemu-system-aarch64', '-machine', 'virt,gic-version=3', '-cpu', 'max', '-m', '2G', '-smp', '2',
                   '-global', 'virtio-mmio.force-legacy=false', '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot',
                   '-qmp', f'unix:{qmp_path},server=on,wait=off', '-kernel', str(OUT / 'Image'), '-initrd', str(OUT / 'initramfs.cpio'),
                   '-append', 'console=ttyAMA0 wifi.mock=1 wifi.mock.data=veth0 init.exec=/bin/network-qa']
        for index in reversed(range(2)):
            command += ['-netdev', f'user,id=net{index},net=10.0.{index+2}.0/24',
                        '-device', f'virtio-net-device,id=nic{index},netdev=net{index},mac=52:54:00:12:34:0{index}']
        command += ['-object', f'filter-dump,id=capture,netdev=net0,file={pcap}']
        seen = []; injected = False; closed_rx = False; carrier_down = False; carrier_up = False; peer = None
        server.settimeout(0.05)
        with log.open('w') as stream:
            process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic()+100
                while time.monotonic()<deadline:
                    try:
                        payload, peer = server.recvfrom(2048)
                        seen.append(payload.decode(errors='replace'))
                        if payload not in (b'LIVE1',b'LIVE2',b'LIVE3',b'LIVE4',b'LIVE5'):
                            raise AssertionError(f'forbidden old/blocked datagram reached host: {payload!r}')
                        server.sendto(payload,peer)
                    except socket.timeout:
                        pass
                    content=log.read_text(errors='replace')
                    if 'WIRELESS_DATA HOLD' in content and not injected and peer:
                        server.sendto(b'OLD_RX',peer)
                        server.sendto(b'OLD_RX',peer)
                        injected=True
                    if 'WIRELESS_DATA PREAUTH_RX' in content and not closed_rx and peer:
                        server.sendto(b'PREAUTH_RX',peer); closed_rx=True
                    if 'WIRELESS_DATA CARRIER_READY' in content and not carrier_down:
                        with Qmp(str(qmp_path)) as qmp: qmp.execute('set_link', {'name':'nic0','up':False})
                        carrier_down=True
                    if 'WIRELESS_DATA CARRIER_DOWN' in content and not carrier_up:
                        with Qmp(str(qmp_path)) as qmp: qmp.execute('set_link', {'name':'nic0','up':True})
                        carrier_up=True
                    if 'WIRELESS_DATA PASS' in content:
                        process.wait(timeout=10)
                        assert process.returncode == 0
                        assert seen == ['LIVE1','LIVE2','LIVE3','LIVE4','LIVE5'], seen
                        assert closed_rx and b'PREAUTH_RX' in pcap.read_bytes(), 'closed-port RX was not exercised'
                        xids=discovers(pcap)
                        assert len(xids)==5, f'expected exactly five fresh DHCP acquisitions: {xids}'
                        assert all(payload not in pcap.read_bytes() for payload in (b'OLD_TX', b'OLD_ARP', b'BLOCKED'))
                        assert before == source_hashes(), 'sources changed during validation'
                        receipt = {
                            'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'result': 'PASS',
                            'rebuilt': not args.no_build, 'source_sha256': before,
                            'udp_round_trips': seen, 'dhcp_discover_xids': xids,
                            'closed_rx_capture_confirmed': True, 'old_tx_absent_from_capture': True,
                            'command': command, 'physical_radio_tested': False,
                            'rustc': subprocess.check_output(['rustc','-vV'],text=True),
                            'artifacts': {name: hashlib.sha256((OUT/name).read_bytes()).hexdigest()
                                          for name in ('Image','initramfs.cpio','qemu.log','veth0.pcap')},
                        }
                        (OUT/'validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
                        print(f'WIRELESS_DATA PASS: UDP={seen}, DHCP discover XIDs={xids}',flush=True)
                        return
                    if process.poll() is not None or any(x in content for x in ('panicked','Panic occurred:','[Scarlet Kernel] panic:')):
                        raise RuntimeError(f'wireless data guest failed; see {log}')
                raise TimeoutError(f'wireless data guest timed out; see {log}')
            finally:
                if process.poll() is None:
                    process.terminate()
                    try: process.wait(timeout=5)
                    except subprocess.TimeoutExpired: process.kill(); process.wait()


if __name__ == '__main__':
    main()
