"""Native-only checks for filesystem semantics behind the SFTP wire interface."""
import argparse,json,os,select,struct,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--artifacts',type=Path,required=True);p.add_argument('--port',type=int,required=True);p.add_argument('--identity',type=Path,required=True);a=p.parse_args();base=a.artifacts.resolve();base.mkdir(exist_ok=True,parents=True)
opts=['-p',str(a.port),'-i',str(a.identity.resolve()),'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=no','-o',f'UserKnownHostsFile={base/"known_hosts"}','-o','ConnectTimeout=10']
command=['/usr/bin/ssh',*opts,'-s','root@127.0.0.1','sftp']
word=lambda n:struct.pack('>I',n)
wide=lambda n:struct.pack('>Q',n)
def string(b):
 if isinstance(b,str):b=b.encode()
 return word(len(b))+b
child=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
request_id=0
def exact(n):
 out=b''
 while len(out)<n:
  assert select.select([child.stdout],[],[],10)[0], 'SFTP response timeout'
  block=os.read(child.stdout.fileno(),n-len(out));assert block,'SFTP EOF';out+=block
 return out
def receive():return exact(struct.unpack('>I',exact(4))[0])
def call(kind,payload=b''):
 global request_id
 request_id+=1;body=bytes([kind])+word(request_id)+payload;child.stdin.write(word(len(body))+body);child.stdin.flush();out=receive();assert out[1:5]==word(request_id);return out
def status(out,code):assert out[0]==101 and out[5:9]==word(code),(out,code)
def open_file(path,flags):
 out=call(3,string(path)+word(flags)+word(0));assert out[0]==102,out;return out[5:]
def write(handle,value,offset=0):status(call(6,handle+wide(offset)+string(value)),0)
def read(handle):
 out=call(5,handle+wide(0)+word(64));assert out[0]==103,out;return out[9:]
def close(handle):status(call(4,handle),0)
checks=[]
try:
 child.stdin.write(word(5)+b'\x01'+word(3));child.stdin.flush();assert receive()[:5]==b'\x02'+word(3)
 root='/root/sftp-native-'+str(time.time_ns());status(call(14,string(root)+word(0)),0)
 ah=open_file(root+'/a',1|2|8|32);write(ah,b'original')
 # An unsupported chmod must not apply the requested truncation either.
 status(call(10,ah+word(1|4)+wide(0)+word(0o777)),8);assert read(ah)==b'original';checks.append('unsupported chmod returns status 8 before truncation')
 status(call(10,ah+word(8)+word(1700000000)+word(1700000010)),0)
 attrs=call(8,ah);assert attrs[0]==105 and attrs[-8:]==word(1700000000)+word(1700000010);checks.append('native file timestamps round trip')
 bh=open_file(root+'/b',2|8|32);write(bh,b'keep');close(bh)
 status(call(18,string(root+'/a')+string(root+'/b')),4)
 bh=open_file(root+'/b',1);assert read(bh)==b'keep'
 status(call(6,bh+wide(0)+string(b'bad')),3);close(bh)
 status(call(200,string('posix-rename@openssh.com')+string(root+'/a')+string(root+'/b')),0)
 write(ah,b'changed!');close(ah);checks.append('no-replace collision, replacement rename, and open file identity')
 ah=open_file(root+'/b',2|4);write(ah,b'tail',0);close(ah)
 ah=open_file(root+'/b',1);assert read(ah)==b'changed!tail';close(ah);checks.append('append ignores positioned write offset')
 directory=root+'/listing';status(call(14,string(directory)+word(0)),0)
 for i in range(70):close(open_file(directory+'/'+str(i),2|8|32))
 out=call(11,string(directory));assert out[0]==102;dh=out[5:];total=0
 while True:
  out=call(12,dh)
  if out[0]==101:status(out,1);break
  assert out[0]==104;count=struct.unpack('>I',out[5:9])[0];assert 0<count<=32;total+=count
 assert total==70;close(dh);checks.append('native directory listing is paged past 32 entries')
 for i in range(70):status(call(13,string(directory+'/'+str(i))),0)
 status(call(15,string(directory)),0);status(call(13,string(root+'/b')),0);status(call(15,string(root)),0)
 child.stdin.close();assert child.wait(timeout=10)==0
finally:
 if child.poll() is None:child.kill();child.wait()
 (base/'native-requests-stderr.log').write_bytes(child.stderr.read())
# Ordinary SSH exec still uses the established shell bridge.
r=subprocess.run(['/usr/bin/ssh',*opts,'root@127.0.0.1','echo SCARLET_SSH_EXEC_OK'],capture_output=True,timeout=15)
(base/'ssh-exec.log').write_bytes(r.stdout+r.stderr);assert r.returncode==0 and b'SCARLET_SSH_EXEC_OK' in r.stdout
checks.append('ordinary SSH exec remains functional')
(base/'native-requests.json').write_text(json.dumps(checks,indent=2)+'\n');print(json.dumps(checks,indent=2))
