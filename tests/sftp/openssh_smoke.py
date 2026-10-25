"""Exercise the production SFTP engine with OpenSSH, directly or over native SSH."""
import argparse,hashlib,json,os,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--artifacts',type=Path,required=True);p.add_argument('--server',type=Path);p.add_argument('--port',type=int);p.add_argument('--identity',type=Path);args=p.parse_args()
base=args.artifacts.resolve();base.mkdir(parents=True,exist_ok=True)
local=base/'local';home=base/'host-home';local.mkdir(exist_ok=True);home.mkdir(exist_ok=True)
payload=bytes(range(256))*4096+b'\x00\xffScarlet SFTP\n\r\x04\x1b'
(local/'payload.bin').write_bytes(payload);(local/'prefix.bin').write_bytes(payload[:70123]);(local/'resumed-download.bin').write_bytes(payload[:55555])
# Separate one remote directory for this invocation; no existing paths are removed.
remote='sftp-check-'+str(time.time_ns())
lines=['pwd',f'mkdir {remote}',f'cd {remote}',f'lcd "{local}"','put -f payload.bin "日本語 file.bin"','ls -l','get "日本語 file.bin" download.bin','rename "日本語 file.bin" renamed.bin','ln -s renamed.bin linked.bin','get linked.bin linked-download.bin','put prefix.bin resumable.bin','reput payload.bin resumable.bin','reget resumable.bin resumed-download.bin','get -f resumable.bin synced-download.bin','mkdir folder','rmdir folder','rm linked.bin','rm renamed.bin','rm resumable.bin','cd ..',f'rmdir {remote}','quit']
batch=base/'batch.sftp';batch.write_text('\n'.join(lines)+'\n')
env=os.environ.copy();cmd=['/usr/bin/sftp','-b',str(batch),'-B','32768','-R','16']
if args.server:
 cmd+=['-D',str(args.server.resolve())];env['SCARLET_SFTP_TEST_HOME']=str(home)
else:
 assert args.port and args.identity
 cmd+=['-P',str(args.port),'-i',str(args.identity.resolve()),'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=no','-o',f'UserKnownHostsFile={base/"known_hosts"}','-o','ConnectTimeout=10','root@127.0.0.1']
start=time.monotonic();result=subprocess.run(cmd,env=env,capture_output=True,timeout=150);elapsed=time.monotonic()-start
(base/'openssh.log').write_bytes(result.stdout+result.stderr)
if result.returncode:raise SystemExit((base/'openssh.log').read_text())
digest=lambda data:hashlib.sha256(data).hexdigest()
report={'transport':'OpenSSH direct server' if args.server else 'OpenSSH SSH subsystem into native Scarlet','bytes':len(payload),'sha256':digest(payload),'elapsed_seconds':round(elapsed,3),'checks':{}}
for name in ['download.bin','linked-download.bin','resumed-download.bin','synced-download.bin']:
 report['checks'][name]=digest((local/name).read_bytes());assert report['checks'][name]==report['sha256'],name
(base/'verification.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
