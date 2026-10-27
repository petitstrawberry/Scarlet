#!/usr/bin/env python3
"""Exercise the real upstream RSN engine against an independent authenticator."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('authenticator',ROOT/'tools/wpa-supplicant/authenticator.py')
a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
spec=importlib.util.spec_from_file_location('wpa_provenance',ROOT/'tools/wpa-supplicant/provenance.py')
provenance=importlib.util.module_from_spec(spec);spec.loader.exec_module(provenance)
OUT=ROOT/'.build/wpa-supplicant'

class Endpoint:
    def __init__(self,binary,password=None,rsnxe=None):
        env=os.environ.copy()
        if password:env['SCARLET_WPA_TEST_PASSWORD']=password
        if rsnxe is not None:env['SCARLET_WPA_TEST_RSNXE']=rsnxe.hex()
        self.p=subprocess.Popen([str(binary)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env=env)
        assert self.p.stdout.readline().strip()=='READY'
    def command(self,line):
        self.p.stdin.write(line+'\n');self.p.stdin.flush();result=[]
        while True:
            text=self.p.stdout.readline().strip()
            if not text:raise AssertionError('RSN endpoint exited: '+self.p.stderr.read())
            result.append(text)
            if text.startswith('RESULT '):return result
    def rx(self,pdu):return self.command('RX '+pdu.hex())
    def close(self):
        self.p.stdin.write('QUIT\n');self.p.stdin.flush();self.p.communicate(timeout=5)
        assert self.p.returncode==0

def tx(lines):return [bytes.fromhex(l[3:]) for l in lines if l.startswith('TX ')]
def check(lines,authorized,live):
    assert f'AUTH {int(authorized)} LIVE {live}' in lines[-1],lines

def run(binary):
    results=[]
    keys=[];nonces=[]
    for number in (1,2):
        ap=a.Authenticator(number);e=Endpoint(binary)
        try:
            m2=tx(e.rx(ap.message1()));assert len(m2)==1;assert ap.message2(m2[0]);nonces.append(ap.snonce)
            lines=e.rx(ap.message3());check(lines,True,2);m4=tx(lines);assert len(m4)==1;ap.message4(m4[0])
            installed={int(l.split()[1]):l.split()[3] for l in lines if l.startswith('KEY ')};assert installed==ap.expected_keys(),installed
            keys.append(installed)
            duplicate=e.rx(ap.message3());check(duplicate,True,2);assert not any(l.startswith('KEY ') for l in duplicate)
            assert 'INSTALLS 2' in duplicate[-1]
            results.append(f'valid_handshake_{number}_and_duplicate_M3')
        finally:e.close()
    assert keys[0]!=keys[1] and nonces[0]!=nonces[1]
    # A valid MIC is insufficient when the M3 security IEs differ from the
    # beacon evidence. Mixed WPA2/WPA3 APs also include RSNXE in that evidence.
    extension=bytes([244,1,0x20])
    for mode in ('match','mismatch','missing','unexpected'):
        ap=a.Authenticator(7);e=Endpoint(binary,rsnxe=extension if mode!='unexpected' else None)
        try:
            assert ap.message2(tx(e.rx(ap.message1()))[0])
            supplied=b'' if mode=='missing' else bytes([244,1,0x60]) if mode=='mismatch' else extension
            lines=e.rx(ap.message3(rsnxe=supplied))
            if mode=='match':
                check(lines,True,2);ap.message4(tx(lines)[0])
            else:
                check(lines,False,0);assert not tx(lines);assert 'Err(' in lines[-1]
            results.append('rsnxe_'+mode)
        finally:e.close()
    for mode in ('bad_mic','wrong_password','bad_nonce','bad_wrap','bad_ie','entropy','key_failure','timeout','truncated'):
        ap=a.Authenticator(3);e=Endpoint(binary,'incorrect password' if mode=='wrong_password' else None)
        try:
            if mode=='entropy':
                e.command('NO_ENTROPY');lines=e.rx(ap.message1());check(lines,False,0);assert not tx(lines);assert 'EntropyUnavailable' in lines[-1]
            else:
                m2=tx(e.rx(ap.message1()));assert len(m2)==1
                verified=ap.message2(m2[0]);assert verified==(mode!='wrong_password')
                if mode=='timeout':lines=e.command('TICK 6000000000')
                elif mode=='truncated':lines=e.rx(ap.message3()[:90])
                else:
                    if mode=='key_failure':e.command('FAIL_KEY')
                    lines=e.rx(ap.message3(bad_mic=mode=='bad_mic',bad_nonce=mode=='bad_nonce',bad_wrap=mode=='bad_wrap',bad_ie=mode=='bad_ie'))
                check(lines,False,0);assert 'Err(' in lines[-1],lines
                if mode!='key_failure':assert not tx(lines),lines
            results.append(mode+'_blocked')
        finally:e.close()
    ap=a.Authenticator(4);e=Endpoint(binary)
    try:
        ap.message2(tx(e.rx(ap.message1()))[0]);ap.message4(tx(e.rx(ap.message3()))[0])
        gtk=bytes(range(16));g1=ap.group1(gtk,2,410,rsc=10)
        lines=e.rx(g1);check(lines,True,2);assert not tx(lines) and not any(l.startswith('KEY ') for l in lines)
        results.append('plaintext_group_downgrade_ignored')
        lines=e.command('RXP '+g1.hex());check(lines,True,2);assert 'PORT 0' not in lines;ap.group2(tx(lines)[0],410)
        assert any(l=='KEY 2 2 '+a.hashlib.sha256(gtk).hexdigest() for l in lines);results.append('protected_group_rekey')
        lines=e.command('RXP '+g1.hex());check(lines,True,2);assert not tx(lines) and 'INSTALLS 3' in lines[-1];results.append('group_replay_ignored')
        retry=ap.group1(gtk,2,411,rsc=1);lines=e.command('RXP '+retry.hex());check(lines,True,2);ap.group2(tx(lines)[0],411)
        assert 'INSTALLS 3' in lines[-1] and not any(l.startswith('KEY ') for l in lines) and 'PORT 0' not in lines;results.append('higher_counter_group_retry_does_not_reinstall')
        lines=e.command('RXP '+ap.group1(bytes(range(16,32)),2,412,rsc=20).hex());check(lines,True,2);ap.group2(tx(lines)[0],412);assert 'INSTALLS 4' in lines[-1];results.append('same_slot_group_cutover')
        lines=e.command('RXP '+ap.group1(ap.gtk,1,413).hex());check(lines,False,0);assert not tx(lines);results.append('retired_gtk_rollback_blocked')
    finally:e.close()
    for mode in ('bad_mic','bad_wrap','key_failure'):
        ap=a.Authenticator(5);e=Endpoint(binary)
        try:
            ap.message2(tx(e.rx(ap.message1()))[0]);ap.message4(tx(e.rx(ap.message3()))[0])
            if mode=='key_failure':e.command('FAIL_KEY')
            g1=ap.group1(bytes(range(16)),2,510,bad_mic=mode=='bad_mic',bad_wrap=mode=='bad_wrap')
            lines=e.command('RXP '+g1.hex());check(lines,False,0);assert not tx(lines);results.append('group_'+mode+'_blocked')
        finally:e.close()
    return results

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--no-build',action='store_true');args=parser.parse_args()
    OUT.mkdir(parents=True,exist_ok=True)
    pin=provenance.upstream();before=provenance.sources()
    host=re.search(r'^host: (.+)$',subprocess.check_output(['rustc','-vV'],text=True),re.M)[1]
    env=os.environ.copy();env['CARGO_TARGET_DIR']=str(OUT/'host-target')
    if not args.no_build:
        with (OUT/'host-build.log').open('w') as log:subprocess.run(['cargo','build','--manifest-path',str(ROOT/'user/lib/scarlet-wpa/Cargo.toml'),'--example','rsn-peer','--release','--target',host],cwd=ROOT,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
    binary=OUT/f'host-target/{host}/release/examples/rsn-peer'
    results=run(binary)
    assert before==provenance.sources(),'sources changed during validation'
    (OUT/'host-validation.json').write_text(json.dumps({'result':'PASS','rebuilt':not args.no_build,'upstream':pin,'source_sha256':before,'toolchain':provenance.toolchain(),'binary_sha256':provenance.sha256(binary),'cases':results,'keys_change_on_reconnect':True,'independent_authenticator':True},indent=2)+'\n')
    print('WPA_HOST PASS:',', '.join(results),flush=True)
if __name__=='__main__':main()
