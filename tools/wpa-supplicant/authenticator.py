"""Independent WPA2-PSK/CCMP test authenticator (stdlib HMAC + OpenSSL AES-wrap).

Not production AP code. Actual EAPOL MICs and encrypted GTK KDEs are generated;
PTK derivation and MIC verification are independent of the vendored supplicant.
"""
import hashlib
import hmac
import struct
import subprocess

SSID = b'ScarletMock'
PASSWORD = b'correct password'
AP = bytes([2, 0, 0, 0, 0, 1])
STA = bytes.fromhex('525400123400')
RSN_IE = bytes.fromhex('30140100000fac040100000fac040100000fac020000')
MIC_OFFSET = 81


def prf(pmk, data):
    return b''.join(hmac.new(pmk, b'Pairwise key expansion\0'+data+bytes([i]), 'sha1').digest() for i in range(4))[:48]


def packet(info, replay, nonce, data=b'', kck=None, rsc=0):
    body = bytes([2])+struct.pack('!HHQ',info,16,replay)+nonce+bytes(16+8+8+16)+struct.pack('!H',len(data))+data
    result = bytearray(bytes([2,3])+struct.pack('!H',len(body))+body)
    result[65:73]=rsc.to_bytes(8,'little')
    if kck is not None:
        result[MIC_OFFSET:MIC_OFFSET+16] = hmac.new(kck,result,'sha1').digest()[:16]
    return bytes(result)


def wrap_key_data(kek,plain):
    # RFC 3394 with an independent OpenSSL AES-ECB block primitive. New
    # OpenSSL enc versions reject the non-streaming wrap mode directly.
    blocks=[plain[i:i+8] for i in range(0,len(plain),8)];acc=bytes.fromhex('a6'*8)
    for j in range(6):
        for i in range(len(blocks)):
            encrypted=subprocess.check_output(['openssl','enc','-aes-128-ecb','-nopad','-K',kek.hex()],input=acc+blocks[i],stderr=subprocess.PIPE)
            assert len(encrypted)==16
            acc=(int.from_bytes(encrypted[:8],'big')^(len(blocks)*j+i+1)).to_bytes(8,'big')
            blocks[i]=encrypted[8:]
    encrypted=acc+b''.join(blocks)
    return encrypted


def parse(pdu):
    if len(pdu)<99 or pdu[1]!=3 or int.from_bytes(pdu[2:4],'big')+4!=len(pdu):
        raise AssertionError('malformed EAPOL-Key')
    size=int.from_bytes(pdu[97:99],'big')
    assert 99+size==len(pdu)
    return {'info':int.from_bytes(pdu[5:7],'big'),'replay':int.from_bytes(pdu[9:17],'big'),
            'nonce':pdu[17:49],'mic':pdu[MIC_OFFSET:MIC_OFFSET+16],'data':pdu[99:]}


class Authenticator:
    def __init__(self, number=1, password=PASSWORD):
        self.number=number
        self.pmk=hashlib.pbkdf2_hmac('sha1',password,SSID,4096,32)
        # Independent fresh test ANonce/GTK per attempt, deterministic for receipts.
        self.anonce=hashlib.sha256(b'Scarlet AP ANonce'+struct.pack('!Q',number)).digest()
        self.gtk=hashlib.sha256(b'Scarlet AP GTK'+struct.pack('!Q',number)).digest()[:16]
        self.replay=number*100
        self.ptk=None
        self.snonce=None
        self.m2_valid=False

    def message1(self):
        return packet(0x008a,self.replay+1,self.anonce)

    def message2(self,pdu):
        decoded=parse(pdu)
        assert decoded['info']==0x010a,hex(decoded['info'])
        assert decoded['replay']==self.replay+1
        assert decoded['data']==RSN_IE
        self.snonce=decoded['nonce']
        assert self.snonce!=bytes(32)
        data=min(AP,STA)+max(AP,STA)+min(self.anonce,self.snonce)+max(self.anonce,self.snonce)
        self.ptk=prf(self.pmk,data)
        self.m2_valid=self.valid_mic(pdu)
        return self.m2_valid

    def valid_mic(self,pdu):
        tmp=bytearray(pdu);mic=bytes(tmp[MIC_OFFSET:MIC_OFFSET+16]);tmp[MIC_OFFSET:MIC_OFFSET+16]=bytes(16)
        return hmac.compare_digest(mic,hmac.new(self.ptk[:16],tmp,'sha1').digest()[:16])

    def message3(self, bad_mic=False, bad_nonce=False, bad_wrap=False, bad_ie=False, rsnxe=b''):
        assert self.ptk is not None
        ie=bytearray(RSN_IE)
        if bad_ie:ie[-1]=1
        plain=bytes(ie)+rsnxe+b'\xdd\x16\x00\x0f\xac\x01\x01\x00'+self.gtk
        if len(plain)%8: plain+=b'\xdd'+bytes((-len(plain)-1)%8)
        assert len(plain)%8==0
        encrypted=wrap_key_data(self.ptk[16:32],plain)
        if bad_wrap:encrypted=bytes([encrypted[0]^1])+encrypted[1:]
        nonce=bytes([self.anonce[0]^1])+self.anonce[1:] if bad_nonce else self.anonce
        result=packet(0x13ca,self.replay+2,nonce,encrypted,self.ptk[:16])
        if bad_mic:result=result[:MIC_OFFSET]+bytes([result[MIC_OFFSET]^1])+result[MIC_OFFSET+1:]
        return result

    def message4(self,pdu):
        decoded=parse(pdu)
        assert decoded['info']==0x030a,hex(decoded['info'])
        assert decoded['replay']==self.replay+2
        assert self.valid_mic(pdu),'M4 MIC mismatch'
        assert not decoded['data']

    def group1(self,gtk,index,replay,rsc=0,bad_mic=False,bad_wrap=False):
        assert len(gtk)==16 and 0<=index<=3
        plain=b'\xdd\x16\x00\x0f\xac\x01'+bytes([index,0])+gtk
        encrypted=wrap_key_data(self.ptk[16:32],plain)
        if bad_wrap:encrypted=bytes([encrypted[0]^1])+encrypted[1:]
        result=packet(0x1382,replay,bytes(32),encrypted,self.ptk[:16],rsc)
        if bad_mic:result=result[:MIC_OFFSET]+bytes([result[MIC_OFFSET]^1])+result[MIC_OFFSET+1:]
        return result

    def group2(self,pdu,replay):
        decoded=parse(pdu);assert decoded['info']==0x0302
        assert decoded['replay']==replay and not decoded['data']
        assert self.valid_mic(pdu),'G2 MIC mismatch'

    def expected_keys(self):
        return {1:hashlib.sha256(self.ptk[32:48]).hexdigest(),2:hashlib.sha256(self.gtk).hexdigest()}
