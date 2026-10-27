#!/usr/bin/env python3
"""QEMU EAPOL, confirmed key and controlled-port lifecycle over virtio/slirp."""
import argparse
import importlib.util
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
import socket
import select
import struct
import subprocess
import sys
import time
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('network_management', ROOT / 'tools/test-network-management.py')
network = importlib.util.module_from_spec(spec)
spec.loader.exec_module(network)
OUT = ROOT / '.build/wireless-auth'
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
        'kernel/Cargo.toml', 'kernel/targets/aarch64-unknown-none-elf.json',
        'tools/network-management/guest/kernel.ld',
        'kernel/src/device/network/mod.rs', 'kernel/src/drivers/network/virtio_net.rs',
        'kernel/src/network/mod.rs', 'kernel/src/network/ethernet.rs', 'kernel/src/network/ethernet_interface.rs',
        'kernel/src/network/arp.rs', 'kernel/src/network/packet.rs', 'kernel/src/network/link.rs',
        'kernel/src/network/config.rs', 'user/lib/scarlet-abi/src/wireless.rs',
        'user/lib/scarlet-os/src/wireless.rs', 'tools/wireless-auth/guest/src/main.rs',
        'tools/wireless-auth/guest/Cargo.toml', 'tools/wireless-auth/guest/Cargo.lock', 'tools/test-wireless-auth.py', 'tools/test-network-management.py')]
    sources += list((ROOT / 'user/bin/src/netcfgd').glob('*.rs'))
    def source_hashes():
        return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(sources)}
    before = source_hashes()
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('127.0.0.1', 0))
        port = server.getsockname()[1]
        port_entry = [('etc/wireless-data-port', 0o100644, str(port).encode())]
        if not args.no_build:
            network.build(wireless_auth=True, extra_entries=port_entry)
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
                   '-append', 'console=ttyAMA0 wifi.mock=1 wifi.mock.data=veth0 wifi.mock.auth=1 init.exec=/bin/network-qa']
        for index in reversed(range(2)):
            command += ['-netdev', f'user,id=net{index},net=10.0.{index+2}.0/24',
                        '-device', f'virtio-net-device,id=nic{index},netdev=net{index},mac=52:54:00:12:34:0{index}']
        command += ['-object', f'filter-dump,id=capture,netdev=net0,file={pcap}']
        tx_path=OUT/'eapol-tx.sock'; rx_path=OUT/'eapol-rx.sock'
        listeners=[]
        for path in (tx_path,rx_path):
            path.unlink(missing_ok=True)
            listener=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);listener.bind(str(path));listener.listen(1);listener.settimeout(10);listeners.append(listener)
        command += ['-chardev',f'socket,id=eapol_tx,path={tx_path}',
                    '-chardev',f'socket,id=eapol_rx,path={rx_path}',
                    '-object','filter-mirror,id=eapol_mirror,netdev=net0,queue=rx,outdev=eapol_tx',
                    '-object','filter-redirector,id=eapol_input,netdev=net0,queue=tx,indev=eapol_rx,position=head']
        eapol_echoes=0; raw_pending=bytearray(); channels=[]
        seen = []; injected = False; closed_rx = False; carrier_down = False; carrier_up = False; peer = None
        server.settimeout(0.05)
        with log.open('w') as stream:
            process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT)
            try:
                channels=[listener.accept()[0] for listener in listeners]
                channels[0].setblocking(False)
                deadline = time.monotonic()+100
                while time.monotonic()<deadline:
                    try:
                        payload, peer = server.recvfrom(2048)
                        seen.append(payload.decode(errors='replace'))
                        if payload not in (b'LIVE1',b'REKEY',b'LIVE2',b'LIVE3',b'LIVE4',b'LIVE5',b'PORT_REOPEN'):
                            raise AssertionError(f'forbidden old/blocked datagram reached host: {payload!r}')
                        server.sendto(payload,peer)
                    except socket.timeout:
                        pass
                    if select.select([channels[0]],[],[],0)[0]:
                        chunk=channels[0].recv(65536)
                        if chunk: raw_pending.extend(chunk)
                    while len(raw_pending)>=4:
                        size=struct.unpack_from('!I',raw_pending)[0]
                        assert 0<size<65536,size
                        if len(raw_pending)<4+size:break
                        frame=bytes(raw_pending[4:4+size]);del raw_pending[:4+size]
                        if len(frame)<18 or frame[12:14]!=b'\x88\x8e':continue
                        assert frame[:6]==bytes([2,0,0,0,0,1]),'wrong EAPOL TX peer'
                        length=4+int.from_bytes(frame[16:18],'big');pdu=frame[14:14+length]
                        assert len(pdu)==12,pdu
                        # Wrong-source EAPOL must be filtered before the owner inbox.
                        wrong=frame[6:12]+bytes([2,0,0,0,0,9])+b'\x88\x8e'+bytes([2,3,0,8])+b'BAD_PEER'
                        reply=frame[6:12]+frame[:6]+b'\x88\x8e'+pdu
                        for packet in (wrong,reply):channels[1].sendall(struct.pack('!I',len(packet))+packet)
                        eapol_echoes+=1
                    content=log.read_text(errors='replace')
                    if 'WIRELESS_AUTH HOLD' in content and not injected and peer:
                        server.sendto(b'OLD_RX',peer)
                        server.sendto(b'OLD_RX',peer)
                        injected=True
                    if 'WIRELESS_AUTH PREAUTH_RX' in content and not closed_rx and peer:
                        server.sendto(b'PREAUTH_RX',peer); closed_rx=True
                    if 'WIRELESS_AUTH CARRIER_READY' in content and not carrier_down:
                        with Qmp(str(qmp_path)) as qmp: qmp.execute('set_link', {'name':'nic0','up':False})
                        carrier_down=True
                    if 'WIRELESS_AUTH CARRIER_DOWN' in content and not carrier_up:
                        with Qmp(str(qmp_path)) as qmp: qmp.execute('set_link', {'name':'nic0','up':True})
                        carrier_up=True
                    if 'WIRELESS_AUTH PASS' in content:
                        process.wait(timeout=10)
                        assert process.returncode == 0
                        assert seen == ['LIVE1','REKEY','LIVE2','LIVE3','LIVE4','LIVE5','PORT_REOPEN'], seen
                        assert closed_rx and b'PREAUTH_RX' in pcap.read_bytes(), 'closed-port RX was not exercised'
                        assert eapol_echoes==6, eapol_echoes
                        xids=discovers(pcap)
                        assert len(xids)==7, f'expected exactly seven fresh DHCP acquisitions: {xids}'
                        assert all(payload not in pcap.read_bytes() for payload in (b'OLD_TX', b'OLD_ARP', b'BLOCKED', b'REKEY_BLOCKED', b'PORT_CLOSED'))
                        assert before == source_hashes(), 'sources changed during validation'
                        receipt = {
                            'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'result': 'PASS',
                            'rebuilt': not args.no_build, 'source_sha256': before,
                            'udp_round_trips': seen, 'dhcp_discover_xids': xids,
                            'closed_rx_capture_confirmed': True, 'old_tx_absent_from_capture': True,
                            'command': command, 'physical_radio_tested': False,
                            'eapol_round_trips': eapol_echoes,
                            'rustc': subprocess.check_output(['rustc','-vV'],text=True),
                            'artifacts': {name: hashlib.sha256((OUT/name).read_bytes()).hexdigest()
                                          for name in ('Image','initramfs.cpio','qemu.log','veth0.pcap')},
                        }
                        (OUT/'validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
                        print(f'WIRELESS_AUTH PASS: EAPOL={eapol_echoes}, UDP={seen}, DHCP discover XIDs={xids}',flush=True)
                        return
                    if process.poll() is not None or any(x in content for x in ('panicked','Panic occurred:','[Scarlet Kernel] panic:')):
                        time.sleep(0.25)
                        raise RuntimeError(f'wireless authentication guest failed; see {log}')
                raise TimeoutError(f'wireless data guest timed out; see {log}')
            finally:
                for channel in channels:channel.close()
                for listener in listeners:listener.close()
                for path in (tx_path,rx_path):path.unlink(missing_ok=True)
                if process.poll() is None:
                    process.terminate()
                    try: process.wait(timeout=5)
                    except subprocess.TimeoutExpired: process.kill(); process.wait()


if __name__ == '__main__':
    main()
