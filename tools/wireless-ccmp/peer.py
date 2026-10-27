"""Independent test AP CCMP peer using libcrypto's CCM, never RustCrypto."""
import ctypes as c
import ctypes.util
import struct
import os
import re
import shutil
import subprocess
import sys

class CCM:
    def __init__(self):
        path=os.environ.get('SCARLET_CCMP_LIBCRYPTO')
        if not path and sys.platform=='darwin':
            deps=subprocess.check_output(['otool','-L',shutil.which('openssl')],text=True)
            path=next((line.strip().split()[0] for line in deps.splitlines() if '/libcrypto.' in line),None)
        if not path:path=ctypes.util.find_library('crypto')
        if not path: raise RuntimeError('libcrypto is required for the independent AP')
        self.path=path;self.lib=c.CDLL(path)
        self.lib.EVP_CIPHER_CTX_new.restype=c.c_void_p
        self.lib.EVP_CIPHER_CTX_free.argtypes=[c.c_void_p]
        self.lib.EVP_aes_128_ccm.restype=c.c_void_p
        self.lib.EVP_CIPHER_CTX_ctrl.argtypes=[c.c_void_p,c.c_int,c.c_int,c.c_void_p]
        for op in ('Encrypt','Decrypt'):
            getattr(self.lib,'EVP_'+op+'Init_ex').argtypes=[c.c_void_p,c.c_void_p,c.c_void_p,c.c_void_p,c.c_void_p]
            getattr(self.lib,'EVP_'+op+'Update').argtypes=[c.c_void_p,c.c_void_p,c.POINTER(c.c_int),c.c_void_p,c.c_int]
        self.lib.EVP_EncryptFinal_ex.argtypes=[c.c_void_p,c.c_void_p,c.POINTER(c.c_int)]
    def crypt(self,key,nonce,aad,payload,tag=None):
        assert len(key)==16 and len(nonce)==13
        ctx=self.lib.EVP_CIPHER_CTX_new();assert ctx
        encrypt=tag is None;prefix='EVP_Encrypt' if encrypt else 'EVP_Decrypt'
        init=getattr(self.lib,prefix+'Init_ex');update=getattr(self.lib,prefix+'Update')
        ctrl=self.lib.EVP_CIPHER_CTX_ctrl;n=c.c_int();buf=c.create_string_buffer(len(payload)+16)
        tagbuf=c.create_string_buffer(8 if encrypt else tag)
        try:
            assert init(ctx,self.lib.EVP_aes_128_ccm(),None,None,None)==1
            assert ctrl(ctx,9,13,None)==1
            assert ctrl(ctx,17,8,None if encrypt else tagbuf)==1
            assert init(ctx,None,None,c.create_string_buffer(key),c.create_string_buffer(nonce))==1
            assert update(ctx,None,c.byref(n),None,len(payload))==1
            assert update(ctx,None,c.byref(n),c.create_string_buffer(aad),len(aad))==1
            valid=update(ctx,buf,c.byref(n),c.create_string_buffer(payload),len(payload))
            if not valid: raise ValueError('CCMP MIC mismatch')
            result=buf.raw[:n.value]
            if encrypt:
                assert self.lib.EVP_EncryptFinal_ex(ctx,buf,c.byref(n))==1
                assert ctrl(ctx,16,8,tagbuf)==1
                return result+tagbuf.raw[:8]
            return result
        finally:self.lib.EVP_CIPHER_CTX_free(ctx)
    def selftest(self):
        # RFC 3610 packet vector #1 (23-byte payload, 8-byte tag).
        key=bytes(range(0xc0,0xd0));nonce=bytes.fromhex('00000003020100a0a1a2a3a4a5');aad=bytes(range(8));plain=bytes(range(8,31))
        result=self.crypt(key,nonce,aad,plain)
        assert result.hex()=='588c979a61c663d2f066d0c2c0f989806d5f6b61dac38417e8d12cfdf926e0'
        assert self.crypt(key,nonce,aad,result[:-8],result[-8:])==plain

CCM_ENGINE=None
def engine():
    global CCM_ENGINE
    if CCM_ENGINE is None:CCM_ENGINE=CCM();CCM_ENGINE.selftest()
    return CCM_ENGINE

def parameters(header,pn,tid):
    aad=bytes([header[0]&0x8f,header[1]&0xc7])+header[4:22]+bytes([header[22]&15,0])
    if tid is not None:aad+=bytes([tid,0])
    return aad,bytes([tid or 0])+header[10:16]+pn.to_bytes(6,'big')

def protect(key,ethernet,pn,ap,sta,tid=None,index=0,to_ds=False):
    addresses=ap+sta+ethernet[:6] if to_ds else ethernet[:6]+ap+ethernet[6:12]
    header=bytes([0x88 if tid is not None else 8,0x41 if to_ds else 0x42,0,0])+addresses+bytes(2)
    if tid is not None:header+=bytes([tid,0])
    b=pn.to_bytes(6,'little');iv=b[:2]+bytes([0,0x20|(index<<6)])+b[2:]
    aad,nonce=parameters(header,pn,tid)
    return header+iv+engine().crypt(key,nonce,aad,b'\xaa\xaa\x03\0\0\0'+ethernet[12:])

def unprotect(key,mpdu,ap,sta):
    assert mpdu[0] in (8,0x88) and mpdu[1]&0xc7==0x41
    assert mpdu[4:10]==ap and mpdu[10:16]==sta
    tid=mpdu[24]&15 if mpdu[0]==0x88 else None;size=26 if tid is not None else 24
    iv=mpdu[size:size+8];assert iv[2:4]==b'\0\x20'
    pn=int.from_bytes(iv[:2]+iv[4:8],'little');aad,nonce=parameters(mpdu[:size],pn,tid)
    data=engine().crypt(key,nonce,aad,mpdu[size+8:-8],mpdu[-8:]);assert data[:6]==b'\xaa\xaa\x03\0\0\0'
    return mpdu[16:22]+sta+data[6:],pn
