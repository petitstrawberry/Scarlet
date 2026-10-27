#!/usr/bin/env python3
"""Reproduce static CCMP fixtures with independent EVP CCM; --write updates them."""
import argparse
from pathlib import Path
import sys
sys.dont_write_bytecode=True
from peer import protect, engine

def vectors():
    engine()  # RFC 3610 self-test before constructing any frame.
    ap=bytes.fromhex('020000000001');sta=bytes.fromhex('525400123400');src=bytes.fromhex('021122334455')
    ethernet=sta+src+b'\x08\0'+bytes(range(40));group=bytes.fromhex('01005e000001')+src+b'\x08\0'+bytes(range(40));tx=src+sta+b'\x08\0'+bytes(range(40))
    tk=bytes(range(16));gtk=bytes(range(16,32));rows=[]
    cases=[('rx',tk,ethernet,1,None,0,False),('rxq0',tk,ethernet,2,0,0,False),('rxq7',tk,ethernet,1,7,0,False),('group',gtk,group,3,3,1,False),('tx',tk,tx,1,None,0,True),('txq',tk,tx,1,7,0,True)]
    eapol=sta+ap+b'\x88\x8e'+bytes([2,3,0,4,1,2,3,4]);group_eapol=b'\xff'*6+ap+eapol[12:]
    cases += [('eapol',tk,eapol,1,None,0,False),('group-eapol',gtk,group_eapol,3,3,1,False)]
    for name,first,pn in [('rekey-group',42,11),('rekey-old-future',42,50),('rekey-group2',43,21)]:
        key=bytes([first])+gtk[1:];cases.append((name,key,group,pn,3,2,False))
    for name,key,eth,pn,tid,index,to_ds in cases:
        rows.append(name+' '+eth.hex()+' '+protect(key,eth,pn,ap,sta,tid,index,to_ds).hex())
    return '# Independent libcrypto CCM vectors. key TK=00..0f / GTK=10..1f; fixed AP/STA.\n'+'\n'.join(rows)+'\n'

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--write',action='store_true');args=p.parse_args();path=Path(__file__).with_name('vectors.txt');expected=vectors()
    if args.write:path.write_text(expected)
    assert path.read_text()==expected,'CCMP fixture differs from independent EVP CCM'
    print('Independent CCMP vectors PASS (11 frames; RFC 3610 self-test)')
if __name__=='__main__':main()
