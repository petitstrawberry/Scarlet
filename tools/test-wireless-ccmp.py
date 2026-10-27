#!/usr/bin/env python3
"""Actual Native WPA2 + software CCMP, bridged to slirp by an independent AP."""
import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import select
import socket
import struct
import subprocess
import sys
import time
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parents[1]
def module(name,path):
    spec=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
network=module('network_management',ROOT/'tools/test-network-management.py')
wpa=module('wpa_peer',ROOT/'tools/wpa-supplicant/authenticator.py')
ccmp=module('ccmp_peer',ROOT/'tools/wireless-ccmp/peer.py')
provenance=module('wpa_provenance',ROOT/'tools/wpa-supplicant/provenance.py')
from qmp import Qmp
OUT=ROOT/'.build/wireless-ccmp';network.OUT=OUT

def pcap_header():return struct.pack('<IHHIIII',0xa1b2c3d4,2,4,0,0,65535,1)
def record(stream,frame):
    now=time.time();stream.write(struct.pack('<IIII',int(now),int(now%1*1e6),len(frame),len(frame))+frame);stream.flush()
def udp_frame(message,group=False):
    udp=struct.pack('!HHHH',40004,40003,8+len(message),0)+message
    ip=bytearray(bytes.fromhex('4500000000000000401100000a0002020a00020f'));ip[2:4]=struct.pack('!H',20+len(udp))
    words=sum(struct.unpack('!10H',ip));words=(words&65535)+(words>>16);words=(words&65535)+(words>>16);ip[10:12]=struct.pack('!H',words^65535)
    return (b'\xff'*6 if group else wpa.STA)+bytes.fromhex('52550a000202')+b'\x08\0'+ip+udp

def exercise(port,server,rekey=False):
    qmp_path=OUT/'qmp.sock';qmp_path.unlink(missing_ok=True);log=OUT/'qemu.log'
    command=['qemu-system-aarch64','-machine','virt,gic-version=3','-cpu','max','-m','2G','-smp','2','-global','virtio-mmio.force-legacy=false','-display','none','-monitor','none','-serial','stdio','-no-reboot','-qmp',f'unix:{qmp_path},server=on,wait=off','-kernel',str(OUT/'Image'),'-initrd',str(OUT/'initramfs.cpio'),'-append','console=ttyAMA0 wifi.mock=1 wifi.mock.data=veth0 wifi.mock.ccmp=1 init.exec=/bin/network-qa','-object','rng-random,id=entropy,filename=/dev/urandom','-device','virtio-rng-device,rng=entropy']
    for i in reversed(range(2)):command+=['-netdev',f'user,id=net{i},net=10.0.{i+2}.0/24','-device',f'virtio-net-device,id=nic{i},netdev=net{i},mac=52:54:00:12:34:0{i}']
    with ExitStack() as resources:
        listeners=[]
        for name in ('to_net','from_net','from_guest','to_guest'):
            path=OUT/(name+'.sock');path.unlink(missing_ok=True);resources.callback(path.unlink,missing_ok=True)
            listener=resources.enter_context(socket.socket(socket.AF_UNIX,socket.SOCK_STREAM));listener.bind(str(path));listener.listen(1);listener.settimeout(20);listeners.append(listener)
            command+=['-chardev',f'socket,id={name},path={path}']
        # RX walks the filter chain backwards, TX forwards. Injection resumes
        # after its filter, so neither plaintext nor ciphertext is intercepted twice.
        for name,direction,kind in [('to_net','rx','indev'),('from_net','tx','outdev'),('from_guest','rx','outdev'),('to_guest','tx','indev')]:
            command+=['-object',f'filter-redirector,id=f_{name},netdev=net0,queue={direction},{kind}={name}']
        wire=resources.enter_context((OUT/'wire.pcap').open('wb'));wire.write(pcap_header())
        bridge=resources.enter_context((OUT/'bridge.pcap').open('wb'));bridge.write(pcap_header())
        stream=resources.enter_context(log.open('w'));process=subprocess.Popen(command,stdout=stream,stderr=subprocess.STDOUT)
        try:
            channels=[resources.enter_context(listener.accept()[0]) for listener in listeners]
            for n in (1,2):channels[n].setblocking(False)
            buffers={1:bytearray(),2:bytearray()}
            def send(channel,frame):
                channels[channel].sendall(struct.pack('!I',len(frame))+frame)
                if channel==3:record(wire,frame)
                if channel==0:record(bridge,frame)
            def inject(mpdu):send(3,wpa.STA+wpa.AP+b'\x88\xb5'+mpdu)
            current=None;cases={};processed=set();seen=[];actions=set();old_m3=None;old_probe=None;old_future=None;rx_pn=0;group_pn=0;group=None;rounds={};initial_gtk=None
            server.setblocking(False);deadline=time.monotonic()+100
            while time.monotonic()<deadline:
                content=log.read_text(errors='replace')
                for label,identity in re.findall(r'CCMP_QA ASSOC (\S+) (\d+)',content):
                    if (label,identity) in processed:continue
                    processed.add((label,identity));current=wpa.Authenticator(len(processed));current.label=label;rx_pn=0;group_pn=0
                    cases[label]={'connection':int(identity),'tx_pns':[],'m2':False,'m4':False}
                    send(3,wpa.STA+wpa.AP+b'\x88\x8e'+current.message1())
                for label,identity in re.findall(r'CCMP_QA MANUAL (\S+) (\d+)',content):
                    if (label,identity) in processed:continue
                    processed.add((label,identity));current=wpa.Authenticator(len(processed));current.label=label;rx_pn=0;group_pn=0
                    variant=42 if label=='held-new' else 0
                    tk=bytearray(range(16));gtk=bytearray(range(16,32));tk[0]^=variant;gtk[0]^=variant
                    current.ptk=bytes(32)+bytes(tk);current.gtk=bytes(gtk)
                    cases[label]={'connection':int(identity),'tx_pns':[],'m2':False,'m4':False,'manual_keys':True}
                readable=select.select([channels[1],channels[2],server],[],[],0.01)[0]
                for channel in (1,2):
                    if channels[channel] not in readable:continue
                    chunk=channels[channel].recv(65536)
                    if not chunk:
                        assert 'CCMP_QA PASS' in log.read_text(errors='replace'),'QEMU raw channel closed before PASS'
                        continue
                    buffers[channel].extend(chunk)
                    buf=buffers[channel]
                    while len(buf)>=4:
                        size=struct.unpack_from('!I',buf)[0];assert 0<size<65536
                        if len(buf)<4+size:break
                        frame=bytes(buf[4:4+size]);del buf[:4+size]
                        if channel==1:
                            if current is None or not (cases[current.label]['m4'] or cases[current.label].get('manual_keys')):continue
                            multicast=frame[0]&1
                            if multicast:group_pn+=1;pn=group_pn;key=current.gtk;index=1
                            else:rx_pn+=1;pn=rx_pn;key=current.ptk[32:48];index=0
                            record(bridge,frame);inject(ccmp.protect(key,frame,pn,wpa.AP,wpa.STA,index=index))
                        else:
                            record(wire,frame)
                            assert current is not None
                            protected=frame[12:14]==b'\x88\xb5'
                            eth=frame
                            if protected:
                                eth,pn=ccmp.unprotect(current.ptk[32:48],frame[14:],wpa.AP,wpa.STA)
                                pns=cases[current.label]['tx_pns'];assert pn>pns[-1] if pns else pn==1;pns.append(pn)
                            elif frame[12:14]!=b'\x88\x8e':raise AssertionError('plaintext ordinary data escaped controlled port')
                            if eth[12:14]==b'\x88\x8e':
                                pdu=eth[14:14+4+int.from_bytes(eth[16:18],'big')];message=wpa.parse(pdu)
                                if message['info']==0x010a:
                                    assert not protected and current.message2(pdu);cases[current.label]['m2']=True
                                    cases[current.label]['tk_sha256']=hashlib.sha256(current.ptk[32:48]).hexdigest()
                                    cases[current.label]['gtk_sha256']=hashlib.sha256(current.gtk).hexdigest()
                                    cases[current.label]['snonce_sha256']=hashlib.sha256(current.snonce).hexdigest()
                                    m3=current.message3()
                                    if current.label=='first':old_m3=m3;initial_gtk=current.gtk
                                    send(3,wpa.STA+wpa.AP+b'\x88\x8e'+m3)
                                elif message['info']==0x030a:
                                    current.message4(pdu);cases[current.label]['m4']=True
                                elif message['info']==0x0302:
                                    assert rekey and protected and group is not None and group['sent_protected']
                                    current.group2(pdu,group['expected_replay']);group['acks']+=1
                                    current.gtk=group['new'];group_pn=max(group_pn if group['activated'] else 0,group['rsc']);group['activated']=True
                                else:raise AssertionError('unexpected EAPOL')
                            else:
                                assert protected
                                send(0,eth)
                if server in readable:
                    payload,address=server.recvfrom(2048);assert payload == bytes([0x5a])*1472 or payload in (b'CCMP_GOOD1',b'CCMP_GOOD2',b'CCMP_GOOD3',b'CCMP_GOOD4',b'HOLD_BASE',b'HOLD_FRESH',b'HOLD_FINAL',b'DURING_GROUP1',b'DURING_GROUP2',b'AFTER_GROUP1',b'AFTER_GROUP2'),payload
                    seen.append('MTU1500' if len(payload)==1472 else payload.decode());server.sendto(payload,address)
                if rekey:
                    def protected_eapol(pdu):
                        return wpa.STA+wpa.AP+b'\x88\x8e'+pdu
                    if 'CCMP_QA GROUP_PLAINTEXT' in content and 'plaintext-group' not in actions:
                        actions.add('plaintext-group');gtk=hashlib.sha256(b'Scarlet rekey GTK'+bytes([1])).digest()[:16]
                        send(3,protected_eapol(current.group1(gtk,2,current.replay+10,rsc=10)))
                    for number in (1,2):
                        tag=f'CCMP_QA GROUP_BEGIN {number}'
                        if tag in content and tag not in actions:
                            actions.add(tag);new=hashlib.sha256(b'Scarlet rekey GTK'+bytes([number])).digest()[:16];rsc=number*10;counter=current.replay+10*number
                            group={'old':current.gtk,'new':new,'index':2,'rsc':rsc,'counter':counter,'expected_replay':counter,'acks':0,'activated':False,'sent_protected':True};rounds[str(number)]=group
                            group['g1']=current.group1(new,2,counter,rsc)
                            rx_pn+=1;group['outer']=ccmp.protect(current.ptk[32:48],protected_eapol(group['g1']),rx_pn,wpa.AP,wpa.STA)
                            inject(group['outer'])
                        tag=f'CCMP_QA GROUP_PENDING {number}'
                        if tag in content and tag not in actions:
                            actions.add(tag);assert group['sent_protected'] and not group['activated'];group_pn+=1
                            inject(ccmp.protect(group['old'],udp_frame(f'GROUP_OLD{number}'.encode(),True),group_pn,wpa.AP,wpa.STA,index=1 if number==1 else 2))
                        tag=f'CCMP_QA GROUP_COMMITTED {number}'
                        if tag in content and tag not in actions and group['activated']:
                            actions.add(tag);group_pn+=1
                            group['probe']=ccmp.protect(group['new'],udp_frame(f'GROUP_NEW{number}'.encode(),True),group_pn,wpa.AP,wpa.STA,index=2);inject(group['probe']);inject(group['probe'])
                            inject(ccmp.protect(group['new'],udp_frame(b'GROUP_FLOOR',True),group['rsc'],wpa.AP,wpa.STA,index=2))
                            inject(ccmp.protect(group['old'],udp_frame(b'OLD_GTK',True),1<<40,wpa.AP,wpa.STA,index=1 if number==1 else 2))
                            inject(ccmp.protect(group['old'],udp_frame(b'OLD_GTK_SAME_SLOT',True),(1<<40)+1,wpa.AP,wpa.STA,index=2))
                        tag=f'CCMP_QA GROUP_RETRY {number}'
                        if tag in content and tag not in actions:
                            actions.add(tag);inject(group['outer'])
                            rx_pn+=1;inject(ccmp.protect(current.ptk[32:48],protected_eapol(group['g1']),rx_pn,wpa.AP,wpa.STA))
                            group['expected_replay']=group['counter']+1
                            retry=current.group1(group['new'],2,group['expected_replay'],rsc=0)
                            rx_pn+=1;inject(ccmp.protect(current.ptk[32:48],protected_eapol(retry),rx_pn,wpa.AP,wpa.STA));inject(group['probe'])
                    if 'CCMP_QA GROUP_ROLLBACK' in content and 'rollback' not in actions:
                        actions.add('rollback');pdu=current.group1(initial_gtk,1,current.replay+90);rx_pn+=1;inject(ccmp.protect(current.ptk[32:48],protected_eapol(pdu),rx_pn,wpa.AP,wpa.STA))
                    if 'CCMP_QA GROUP_BAD_MIC' in content and 'bad-group' not in actions:
                        actions.add('bad-group');pdu=current.group1(bytes(range(16)),2,current.replay+10,bad_mic=True);rx_pn+=1;inject(ccmp.protect(current.ptk[32:48],protected_eapol(pdu),rx_pn,wpa.AP,wpa.STA))
                if 'CCMP_QA INJECT' in content and 'inject' not in actions:
                    actions.add('inject');tk=current.ptk[32:48]
                    def probe(message,pn,tid=None,group=False,index=0):return ccmp.protect(current.gtk if group else tk,udp_frame(message,group),pn,wpa.AP,wpa.STA,tid,index)
                    old_probe=probe(b'RX_NQ',100);old_future=probe(b'OLD_KEY',1<<40)
                    p0=probe(b'RX_Q0',101,0);p7=probe(b'RX_Q7',1,7);pg=probe(b'RX_GROUP',1,3,True,1)
                    inject(old_probe);inject(p0);inject(p7);inject(pg)
                    inject(old_probe);inject(p0);inject(p7);inject(pg);inject(probe(b'ZERO_PN',0,7))
                    bad=bytearray(probe(b'BAD_MIC',1000));bad[-1]^=1;inject(bad)
                    inject(probe(b'RX_AFTER_BAD',102));rx_pn=102
                    bad=bytearray(probe(b'BAD_TID',2,7));bad[24]=8;inject(bad);inject(probe(b'RX_Q8',1,8))
                    send(3,udp_frame(b'PLAINTEXT'));inject(probe(b'WRONG_SLOT',2,3,True,2))
                if 'CCMP_QA HOLD_RX' in content and 'held-rx' not in actions:
                    actions.add('held-rx');rx_pn+=1;inject(ccmp.protect(current.ptk[32:48],udp_frame(b'STALE_RX'),rx_pn,wpa.AP,wpa.STA))
                if 'CCMP_QA DUPLICATE' in content and 'duplicate' not in actions:
                    actions.add('duplicate');send(3,wpa.STA+wpa.AP+b'\x88\x8e'+old_m3);inject(old_probe)
                if 'CCMP_QA OLD_KEY' in content and 'oldkey' not in actions:
                    actions.add('oldkey');inject(old_future)
                if 'CCMP_QA CARRIER_READY' in content and 'down' not in actions:
                    actions.add('down')
                    with Qmp(str(qmp_path)) as qmp:qmp.execute('set_link',{'name':'nic0','up':False})
                if 'CCMP_QA CARRIER_DOWN' in content and 'down-inject' not in actions:
                    actions.add('down-inject');inject(old_future)
                if 'CCMP_QA CARRIER_UP' in content and 'up' not in actions:
                    actions.add('up')
                    with Qmp(str(qmp_path)) as qmp:qmp.execute('set_link',{'name':'nic0','up':True})
                if 'CCMP_QA PASS' in content:
                    process.wait(timeout=10);assert process.returncode==0
                    expected=['CCMP_GOOD1','MTU1500','CCMP_GOOD2']+(['DURING_GROUP1','AFTER_GROUP1','DURING_GROUP2','AFTER_GROUP2'] if rekey else [])+['CCMP_GOOD3','CCMP_GOOD4','HOLD_BASE','HOLD_FRESH','HOLD_FINAL']
                    assert seen==expected,seen
                    assert len(cases)==(6 if rekey else 5) and all(c['tx_pns'] and (c.get('manual_keys') or (c['m2'] and c['m4'])) for c in cases.values())
                    assert len({cases[label]['tk_sha256'] for label in ('first','second','automatic')})==3
                    assert len({cases[label]['snonce_sha256'] for label in ('first','second','automatic')})==3
                    assert b'STALE_TX' not in (OUT/'bridge.pcap').read_bytes()
                    expected_actions={'inject','duplicate','oldkey','down','down-inject','up','held-rx'}
                    if rekey:
                        expected_actions|={'rollback','bad-group','plaintext-group'}|{f'CCMP_QA GROUP_{phase} {n}' for phase in ('BEGIN','PENDING','COMMITTED','RETRY') for n in (1,2)}
                        assert all(g['acks']==2 for g in rounds.values()),rounds
                    assert actions==expected_actions
                    assert b'CCMP_GOOD' not in (OUT/'wire.pcap').read_bytes()
                    return {'group_rekey':{n:{k:(hashlib.sha256(v).hexdigest() if isinstance(v,bytes) else v) for k,v in g.items() if k not in ('g1','outer','probe')} for n,g in rounds.items()},'protected_group_rekey_tested':rekey,'cases':cases,'udp':seen,'actions':sorted(actions),'qemu_command':command}
                if process.poll() is not None or any(p in content for p in ('panicked','Panic occurred:','[Scarlet Kernel] panic:')):raise RuntimeError(f'CCMP guest failed; see {log}')
            raise TimeoutError(f'CCMP guest timed out; see {log}')
        finally:
            if process.poll() is None:
                process.terminate()
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:process.kill();process.wait()

def main(rekey=False):
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--no-build',action='store_true');args=parser.parse_args();OUT.mkdir(parents=True,exist_ok=True)
    pin=provenance.upstream();ccmp.engine();before=provenance.sources()
    with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as server:
        server.bind(('127.0.0.1',0));port=server.getsockname()[1]
        entries=[('etc/wpa-good.conf',0o100600,b'network={\nssid="ScarletMock"\npsk="correct password"\n}\n'),('etc/wpa-qa-port',0o100644,str(port).encode())]
        if rekey:entries.append(('etc/wpa-rekey-qa',0o100644,b'1'))
        if not args.no_build:network.build(wireless_ccmp=True,extra_entries=entries)
        else:
            raw=(OUT/'initramfs.cpio').read_bytes();offset=0;records=[]
            while True:
                fields=[int(raw[offset+6+i*8:offset+14+i*8],16) for i in range(13)];start=offset+110;name=raw[start:start+fields[11]-1].decode();start=(start+fields[11]+3)&~3
                payload=raw[start:start+fields[6]];offset=(start+fields[6]+3)&~3
                if name=='TRAILER!!!':break
                if name!='etc/wpa-qa-port':records.append((name,fields[1],payload))
            network.cpio(records+[next(e for e in entries if e[0]=='etc/wpa-qa-port')],OUT/'initramfs.cpio')
        result=exercise(port,server,rekey)
    assert before==provenance.sources(),'source changed during validation'
    receipt={'result':'PASS','rebuilt':not args.no_build,'verified_at_utc':datetime.now(timezone.utc).isoformat(),'upstream':pin,'source_sha256':before,'toolchain':provenance.toolchain(qemu=True),'independent_ccm_library':ccmp.engine().path,'rfc3610_vector':'PASS','physical_radio_tested':False,'ccmp_data_encryption':True,**result}
    receipt['artifacts_sha256']={name:provenance.sha256(OUT/name) for name in ('Image','initramfs.cpio','qemu.log','wire.pcap','bridge.pcap','userspace-target/aarch64-unknown-scarlet/release/wpa-supplicant')}
    (OUT/'validation.json').write_text(json.dumps(receipt,indent=2)+'\n');print('REKEY_QA PASS: protected EAPOL, atomic GTK rotation, data continuity, replay/rollback refusal, monotonic PN.' if rekey else 'CCMP_QA PASS: independent CCM, encrypted DHCP/UDP, replay/MIC isolation, M3 PN continuity, fresh keys and retirement.',flush=True)
if __name__=='__main__':main()
