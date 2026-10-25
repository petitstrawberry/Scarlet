"""Time OpenSSH uploads/downloads, including connection setup, and verify bytes."""
import argparse,hashlib,json,os,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--artifacts',type=Path,required=True);p.add_argument('--port',type=int,required=True);p.add_argument('--identity',type=Path,required=True);p.add_argument('--mib',type=int,default=8);p.add_argument('--payload',type=Path);p.add_argument('--label',required=True);a=p.parse_args()
base=a.artifacts.resolve();base.mkdir(parents=True,exist_ok=True);payload=a.payload.resolve() if a.payload else base/'payload.bin';download=base/'download.bin'
if not a.payload and (not payload.exists() or payload.stat().st_size != a.mib*1024*1024):payload.write_bytes(os.urandom(a.mib*1024*1024))
remote='/root/sftp-benchmark-'+str(time.time_ns())+'.bin'
opts=['/usr/bin/sftp','-vv','-B','32768','-R','64','-P',str(a.port),'-i',str(a.identity.resolve()),'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','Compression=no','-o','StrictHostKeyChecking=no','-o',f'UserKnownHostsFile={base/"known_hosts"}','-o','ConnectTimeout=10']
results={'label':a.label,'bytes':payload.stat().st_size,'timing':'wall time including SSH connection setup','requests':64,'block_bytes':32768}
def run(name,command):
 batch=base/(name+'.sftp');batch.write_text(command+'\nquit\n');start=time.monotonic()
 try:r=subprocess.run([*opts,'-b',str(batch),'root@127.0.0.1'],capture_output=True,timeout=180)
 except subprocess.TimeoutExpired as error:
  (base/(name+'.log')).write_bytes((error.stdout or b'')+(error.stderr or b''))
  results[name]={'timeout_seconds':180,'partial_download_bytes':download.stat().st_size if download.exists() else 0}
  (base/'benchmark.json').write_text(json.dumps(results,indent=2)+'\n');raise
 elapsed=time.monotonic()-start
 (base/(name+'.log')).write_bytes(r.stdout+r.stderr)
 if r.returncode:raise RuntimeError((base/(name+'.log')).read_text())
 return elapsed
try:
 upload=run('upload',f'put "{payload}" {remote}')
 results['upload']={'seconds':round(upload,4),'MB_per_second':round(results['bytes']/upload/1000000,3)}
 (base/'benchmark.json').write_text(json.dumps(results,indent=2)+'\n')
 download_time=run('download',f'get {remote} "{download}"')
 digest=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
 assert digest(payload)==digest(download),'download hash mismatch'
 for name,seconds in [('upload',upload),('download',download_time)]:
  results[name]={'seconds':round(seconds,4),'MB_per_second':round(results['bytes']/seconds/1000000,3),'MiB_per_second':round(results['bytes']/seconds/1048576,3)}
 results['sha256']=digest(payload);(base/'benchmark.json').write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(results,indent=2),flush=True)
finally:run('cleanup',f'-rm {remote}')
