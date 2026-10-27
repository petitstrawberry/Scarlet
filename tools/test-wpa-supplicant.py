#!/usr/bin/env python3
"""Boot actual Native wpa-supplicant and test WPA2-Personal against a raw AP peer."""
import argparse
from contextlib import ExitStack
from datetime import datetime,timezone
import hashlib
import importlib.util
import json
import re
import select
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parents[1]
def module(name,path):
    spec=importlib.util.spec_from_file_location(name,path);obj=importlib.util.module_from_spec(spec);spec.loader.exec_module(obj);return obj
network=module('network_management',ROOT/'tools/test-network-management.py')
raw_data=module('wireless_data',ROOT/'tools/test-wireless-data.py')
ap_code=module('authenticator',ROOT/'tools/wpa-supplicant/authenticator.py')
provenance=module('wpa_provenance',ROOT/'tools/wpa-supplicant/provenance.py')
from qmp import Qmp
OUT=ROOT/'.build/wpa-supplicant';network.OUT=OUT

def profile(password):return f'network={{\nssid="ScarletMock"\npsk="{password}"\nkey_mgmt=WPA-PSK\nproto=RSN\npairwise=CCMP\ngroup=CCMP\n}}\n'.encode()

def exercise(no_entropy,port,server):
    suffix='no-entropy' if no_entropy else 'normal'
    log=OUT/f'qemu-{suffix}.log';pcap=OUT/f'{suffix}.pcap';qmp_path=OUT/'qmp.sock';qmp_path.unlink(missing_ok=True)
    command=['qemu-system-aarch64','-machine','virt,gic-version=3','-cpu','max','-m','2G','-smp','2','-global','virtio-mmio.force-legacy=false','-display','none','-monitor','none','-serial','stdio','-no-reboot','-qmp',f'unix:{qmp_path},server=on,wait=off','-kernel',str(OUT/'Image'),'-initrd',str(OUT/'initramfs.cpio'),'-append','console=ttyAMA0 wifi.mock=1 wifi.mock.data=veth0 wifi.mock.wpa2=1 init.exec=/bin/network-qa']
    for i in reversed(range(2)):command+=['-netdev',f'user,id=net{i},net=10.0.{i+2}.0/24','-device',f'virtio-net-device,id=nic{i},netdev=net{i},mac=52:54:00:12:34:0{i}']
    if not no_entropy:command+=['-object','rng-random,id=entropy,filename=/dev/urandom','-device','virtio-rng-device,rng=entropy']
    command+=['-object',f'filter-dump,id=capture,netdev=net0,file={pcap}']
    with ExitStack() as resources:
        paths=[OUT/'wpa-tx.sock',OUT/'wpa-rx.sock'];listeners=[]
        for path in paths:
            path.unlink(missing_ok=True);resources.callback(path.unlink,missing_ok=True)
            listener=resources.enter_context(socket.socket(socket.AF_UNIX,socket.SOCK_STREAM));listener.bind(str(path));listener.listen(1);listener.settimeout(15);listeners.append(listener)
        command+=['-chardev',f'socket,id=raw_tx,path={paths[0]}','-chardev',f'socket,id=raw_rx,path={paths[1]}','-object','filter-mirror,id=raw_mirror,netdev=net0,queue=rx,outdev=raw_tx','-object','filter-redirector,id=raw_input,netdev=net0,queue=tx,indev=raw_rx,position=head']
        stream=resources.enter_context(log.open('w'));process=subprocess.Popen(command,stdout=stream,stderr=subprocess.STDOUT)
        try:
            channels=[resources.enter_context(listener.accept()[0]) for listener in listeners];channels[0].setblocking(False)
            def inject(pdu):
                frame=ap_code.STA+ap_code.AP+b'\x88\x8e'+pdu;channels[1].sendall(struct.pack('!I',len(frame))+frame)
            buffer=bytearray();current=None;cases={};seen=[];processed=set();duplicate=False;down=False;up=False;service_down=False;service_up=False;old_m3=None
            server.settimeout(0.02);deadline=time.monotonic()+150
            while time.monotonic()<deadline:
                try:
                    payload,peer=server.recvfrom(2048)
                    assert payload in (b'GOOD1',b'GOOD2',b'GOOD3',b'GOOD4',b'GOOD5',b'GOOD6'),f'blocked payload reached host: {payload!r}'
                    seen.append(payload.decode());server.sendto(payload,peer)
                except socket.timeout:pass
                content=log.read_text(errors='replace')
                for label,connection in re.findall(r'WPA_QA ASSOC (\S+) (\d+)',content):
                    marker=(label,connection)
                    if marker in processed:continue
                    processed.add(marker);current=ap_code.Authenticator(len(processed));current.label=label
                    cases[label]={'connection':int(connection),'m2':False,'m4':False}
                    inject(current.message1())
                if select.select([channels[0]],[],[],0)[0]:
                    chunk=channels[0].recv(65536)
                    if chunk:buffer.extend(chunk)
                while len(buffer)>=4:
                    size=struct.unpack_from('!I',buffer)[0];assert 0<size<65536
                    if len(buffer)<4+size:break
                    frame=bytes(buffer[4:4+size]);del buffer[:4+size]
                    if len(frame)<18 or frame[12:14]!=b'\x88\x8e':continue
                    assert current is not None and frame[:6]==ap_code.AP
                    length=4+int.from_bytes(frame[16:18],'big');pdu=frame[14:14+length];message=ap_code.parse(pdu)
                    case=cases[current.label]
                    if message['info']==0x010a:
                        assert not no_entropy,'supplicant sent M2 without trusted entropy'
                        verified=current.message2(pdu);case['m2']=True;case['m2_mic_valid']=verified
                        assert verified==(current.label!='wrong-password')
                        case['snonce_sha256']=hashlib.sha256(current.snonce).hexdigest()
                        case['tk_sha256']=current.expected_keys()[1];case['gtk_sha256']=current.expected_keys()[2]
                        if current.label=='timeout':continue
                        m3=current.message3(bad_mic=current.label=='bad-mic')
                        if current.label=='good':old_m3=m3
                        if current.label=='stale-replay':inject(old_m3)
                        else:inject(m3)
                    elif message['info']==0x030a:
                        assert current.label not in ('bad-mic','wrong-password','timeout','stale-replay','no-entropy')
                        current.message4(pdu);case['m4']=True
                    else:raise AssertionError(f'unexpected STA EAPOL-Key flags: {message["info"]:#x}')
                if 'WPA_QA DUPLICATE' in content and not duplicate:
                    inject(old_m3);duplicate=True
                if 'WPA_QA CARRIER_READY' in content and not down:
                    with Qmp(str(qmp_path)) as qmp:qmp.execute('set_link',{'name':'nic0','up':False})
                    down=True
                if 'WPA_QA CARRIER_DOWN' in content and not up:
                    with Qmp(str(qmp_path)) as qmp:qmp.execute('set_link',{'name':'nic0','up':True})
                    up=True
                if 'WPA_QA SERVICE_CARRIER_READY' in content and not service_down:
                    with Qmp(str(qmp_path)) as qmp:qmp.execute('set_link',{'name':'nic0','up':False})
                    service_down=True
                if 'WPA_QA SERVICE_CARRIER_DOWN' in content and not service_up:
                    with Qmp(str(qmp_path)) as qmp:qmp.execute('set_link',{'name':'nic0','up':True})
                    service_up=True
                marker='WPA_QA NO_ENTROPY PASS' if no_entropy else 'WPA_QA PASS'
                if marker in content:
                    process.wait(timeout=10);assert process.returncode==0
                    if no_entropy:
                        assert 'WPA_QA MODE NO_ENTROPY' in content and not seen and not cases['no-entropy']['m2']
                        assert not raw_data.discovers(pcap)
                    else:
                        assert 'WPA_QA MODE NORMAL' in content
                        assert seen==['GOOD1','GOOD2','GOOD3','GOOD4','GOOD5','GOOD6'],seen
                        assert 'WPA_QA SERVICE_EVENTS PASS' in content
                        assert 'WPA_QA CLI_RENAMED_INTERFACE PASS' in content
                        for label in ('good','reconnect','automatic','service','service-reconnect','service-automatic'):assert cases[label]['m2_mic_valid'] and cases[label]['m4']
                        for label in ('bad-mic','wrong-password','timeout','stale-replay'):assert cases[label]['m2'] and not cases[label]['m4']
                        assert len({cases[label]['snonce_sha256'] for label in ('good','reconnect','automatic','service','service-reconnect','service-automatic')})==6
                        assert len({cases[label]['tk_sha256'] for label in ('good','reconnect','automatic','service','service-reconnect','service-automatic')})==6
                        assert len(raw_data.discovers(pcap))==6
                        assert b'BLOCKED_' not in pcap.read_bytes()
                    return {'mode':suffix,'result':'PASS','udp':seen,'cases':cases,'dhcp_discover_xids':raw_data.discovers(pcap),'command':command,'artifacts':{name:hashlib.sha256((OUT/name).read_bytes()).hexdigest() for name in (log.name,pcap.name,'Image','initramfs.cpio')}}
                if process.poll() is not None or any(x in content for x in ('Panic occurred:','panicked','[Scarlet Kernel] panic:')):
                    time.sleep(0.25);raise RuntimeError(f'WPA guest failed; see {log}')
            raise TimeoutError(f'WPA guest timed out; see {log}')
        finally:
            if process.poll() is None:
                process.terminate()
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:process.kill();process.wait()

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--no-build',action='store_true');args=parser.parse_args();OUT.mkdir(parents=True,exist_ok=True)
    pin=provenance.upstream();before=provenance.sources()
    with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as server:
        server.bind(('127.0.0.1',0));port=server.getsockname()[1]
        entries=[('etc/wpa-good.conf',0o100600,profile('correct password')),('etc/wpa-bad.conf',0o100600,profile('incorrect password')),('etc/wpa-qa-port',0o100644,str(port).encode())]
        if not args.no_build:network.build(wireless_wpa=True,extra_entries=entries)
        else:
            # Only update the ephemeral UDP host port in our own newc archive.
            raw=(OUT/'initramfs.cpio').read_bytes();offset=0;records=[]
            while True:
                fields=[int(raw[offset+6+i*8:offset+14+i*8],16) for i in range(13)];start=offset+110
                name=raw[start:start+fields[11]-1].decode();start=(start+fields[11]+3)&~3
                payload=raw[start:start+fields[6]];offset=(start+fields[6]+3)&~3
                if name=='TRAILER!!!':break
                if name!='etc/wpa-qa-port':records.append((name,fields[1],payload))
            network.cpio(records+[entries[-1]],OUT/'initramfs.cpio')
        results=[exercise(False,port,server),exercise(True,port,server)]
    assert before==provenance.sources(),'sources changed during validation'
    receipt={'verified_at_utc':datetime.now(timezone.utc).isoformat(),'result':'PASS','rebuilt':not args.no_build,'upstream':pin,'source_sha256':before,'toolchain':provenance.toolchain(qemu=True),'daemon_sha256':provenance.sha256(OUT/'userspace-target/aarch64-unknown-scarlet/release/wpa-supplicant'),'physical_radio_tested':False,'runs':results}
    (OUT/'validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
    print('WPA_QA PASS: actual Native daemon, independent MIC/GTK verification, failure isolation, fresh reconnect keys, strict entropy.',flush=True)
if __name__=='__main__':main()
